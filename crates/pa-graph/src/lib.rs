// FORK-ONLY: allspark fork crate; do not push upstream.
//! # pa-graph
//!
//! On-disk SQLite code graph + full-text symbol search, a native Rust port of
//! the `codebase-memory-mcp` store patterns and the `ruvector` hybrid-retrieval
//! techniques, built on `pa-ast` for parsing.
//!
//! ```no_run
//! use pa_graph::{GraphStore, IndexOptions, SearchOptions, TraceOptions};
//! use std::path::Path;
//!
//! let mut store = GraphStore::open("graph.db")?;
//! let report = store.index_project(Path::new("/repo"), &IndexOptions::new("repo"))?;
//! let hits = store.search("parse config", &SearchOptions::new("repo"))?;
//! let trace = store.trace(&TraceOptions::new("repo", "GraphStore::open"))?;
//! # Ok::<(), pa_graph::GraphError>(())
//! ```
//!
//! ## Layout
//!
//! - [`store`] — the [`GraphStore`] connection, node/edge writes, file hashes.
//! - [`schema`] — the SQLite DDL.
//! - [`indexer`] — walk/parse/extract/resolve, incremental classification.
//! - [`extract`] — `pa-ast` tree → symbols, calls, imports.
//! - [`search`] — three-channel BM25/regex/degree retrieval fused by RRF.
//! - [`trace`] — callers/callees, data-flow, cross-service edges.
//! - [`bm25`] / [`hybrid`] — Robertson BM25, RRF, MMR.
//! - [`filter`] — payload-filter expressions compiled to SQL.
//! - [`tokenize`](mod@tokenize) — `camelCase`/`snake_case` identifier splitting.
//! - [`strictness`] — the `ast-grep` strictness ladder.
//! - [`changes`] — git-diff → affected symbols, with branch/co-change provenance.
//! - [`coverage`](mod@coverage) — honest index-coverage accounting (`parse_partial` etc.).
//! - [`deps`] — the workspace's external crate names, for naming dependency calls.
//! - [`architecture`](mod@architecture) — overview/structure/layers/clusters/cycles/hotspots.
//! - [`repair`] — parse-regression auto-repair, accepted only if it re-parses.
//! - [`surface`] / [`incremental`] — surface/body hashes and depth-1 closure
//!   repair (G7): a body-only edit reindexes one file; a signature edit pulls
//!   the file's direct dependents.
//! - [`lsp_surface`] — persisted LSP-surface cache with a reference Bloom
//!   filter, invalidated by surface hash (L11).
//! - [`signals`](mod@signals) — eleven native similarity signals (no external embeddings)
//!   concatenated into one comparable vector (V3).
//! - [`minhash`](mod@minhash) — `MinHash` + `LSH` near-duplicate detection over AST trigrams
//!   (V6).
//! - [`tri_modal`](mod@tri_modal) — ANN + `BM25` + graph-traversal retrieval fused with RRF
//!   (G13).
//! - [`delta`] / [`delta_graph`] — event-sourced change log and delta-aware
//!   graph traversal with `affected_nodes` (R8/R9).
//! - [`delta_index`] — incremental ANN index maintenance with Lazy/Eager/
//!   Batched/Adaptive repair policies (R10).
//! - [`condenser`] — lazy streaming condensation behind a dirty flag (R12).
//! - [`lease`] — the cross-process writer lease (`<db>.index.lock`) that keeps
//!   N sessions off the same tree.
//! - [`fs_watcher`] — mtime-polling and native filesystem watchers emitting
//!   LSP `didChangeWatchedFiles` (+ open/close for created files) (S16).
//!
//! ## Phase 6-A: memory & optimization
//!
//! - [`coherence`] / [`temporal`] — coherence-weighted compaction (M1) and
//!   exponential temporal decay (M2).
//! - [`tarl`] / [`lineage`] — trajectory-aware reward ledger (M4) and record
//!   lineage (M5).
//! - [`witness`] / [`proof_gate`] — hash-linked witness receipts (M6) and the
//!   proof gate that accepts a result only on valid receipts + envelope (R17).
//! - [`bounded_rag`] / [`quality`] — budgeted context selection (M8) and the
//!   quality envelope with degradation flags (M9).
//! - [`query_cache`] / [`witness_cache`] / [`two_random`] — semantic query cache
//!   (M10), witness-keyed cache (M11), 2-random eviction (M3).
//! - [`sparsifier`] / [`ann_repair`] / [`snapcompact`] — graph sparsification
//!   (R13), ANN-graph repair (M7), snapshot compaction (O11).
//! - [`conformal`] / [`self_reconstruct`] — conformal prediction intervals (R18)
//!   and checksum-driven self-reconstruction (R21).
//! - [`adr`] / [`pdx`] — Architecture Decision Records in SQLite (G10) and the
//!   columnar PDX vector layout (V20).
//!
//! ## Phase 8: mux / scopes / masking
//!
//! - [`scoped_context`] — physically isolated per-scope SQLite shards with
//!   fail-closed recovery and quarantine (R1).
//! - [`observation_masking`] — append-only log projected for the model with
//!   dereferenceable recall ids (R7).
//! - [`cypher`] — a read-only Cypher subset compiled to SQL over the graph,
//!   with predicate pushdown and join reordering (G14).
//! - [`prometheus_metrics`] — counters/gauges/histograms with a hand-rolled
//!   exposition format plus health/readiness (R14).
//! - [`mcp_brain`] — an in-process shared collective memory: share/search/vote/
//!   transfer/drift/partition (R20).

#![forbid(unsafe_code)]
// SQLite's row domain is i64 for every INTEGER column and the schema stores
// line numbers, node ids, and counts there. Converting them to the crate's
// `usize`/`u32`/`f32` domain is pervasive and provably in range (ids and lines
// are non-negative and bounded by file/index size), so the cast family is
// allowed crate-wide with that argument stated here rather than at ~30 sites.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

pub mod architecture;
pub mod bm25;
pub mod cascade;
pub mod changes;
pub mod condenser;
pub mod coverage;
pub mod delta;
pub mod delta_graph;
pub mod delta_index;
pub mod deps;
pub mod error;
pub mod extract;
pub mod filter;
pub mod fs_watcher;
pub mod hash;
pub mod hybrid;
pub mod incremental;
pub mod indexer;
pub mod lease;
pub mod lsp_surface;
pub mod minhash;
pub mod misses;
pub mod probe;
pub mod repair;
pub mod schema;
pub mod search;
pub mod signals;
pub mod status;
pub mod store;
pub mod strictness;
pub mod surface;
pub mod tokenize;
pub mod trace;
pub mod tri_modal;
pub mod types;

// ── Phase 6-A: memory & optimization techniques (M1-M11, R13/R17/R18/R21,
// G10, V20, O11) ──────────────────────────────────────────────────────────
pub mod adr;
pub mod ann_repair;
pub mod bounded_rag;
pub mod coherence;
pub mod conformal;
pub mod lineage;
pub mod pdx;
pub mod proof_gate;
pub mod quality;
pub mod query_cache;
pub mod self_reconstruct;
pub mod snapcompact;
pub mod sparsifier;
pub mod tarl;
pub mod temporal;
pub mod two_random;
pub mod witness;
pub mod witness_cache;

// ── Phase 8: mux / scopes / masking (R1, R7) ─────────────────────────────
pub mod cypher;
pub mod mcp_brain;
pub mod observation_masking;
pub mod prometheus_metrics;
pub mod scoped_context;

// ── Phase 6-D: governance / misc (R2, R3, R5, R6, R19, R22, G18) ─────────
pub mod capgated;
pub mod cognitive_container;
pub mod collections;
pub mod homology;
pub mod ns_routing;
pub mod schema_cache;
pub mod voi;

// ── Phase 6-C: ANN algorithms (V10, V13, V15, V16, V17, V19, R11) ─────────
pub mod acorn;
pub mod bounded;
pub mod drifting;
pub mod gnn_rerank;
pub mod lsm_bloom;
pub mod speculative;
pub mod targeted;

pub use architecture::{
    architecture, Architecture, ArchitectureView, Cluster, Cycle, EntryPoint, Hotspot, Layer,
    ModuleEdge, Overview, TreeNode,
};
pub use changes::{
    detect_changes, AffectedSymbol, ChangeReport, ChangeStatus, ChangedFile, GitState, HunkRange,
};
pub use condenser::{Condensation, KeyedCondenser, StreamingCondenser};
pub use coverage::{
    classify_source, coverage, CoverageNote, CoverageReport, ParseHealth, KIND_PARTIAL,
    KIND_SKIPPED, KIND_UNUSABLE,
};
pub use delta::{Change, Delta, DeltaStream, DeltaWindow, Encoding, Op as DeltaOp, Payload};
pub use delta_graph::{
    affected_nodes, Adjacency, AffectedNodes, DeltaAwareTraversal, EdgeDelta, EdgeOp, GraphDelta,
    NodeDelta, NodeOp, TraversalOrder, TraversalResult,
};
pub use delta_index::{AnnGraph, IncrementalUpdater, QualityMonitor, RepairPolicy};
pub use deps::DependencyCrates;
pub use error::{GraphError, Result};
pub use extract::{CallSite, ExtractedFile, ImportDef, SymbolDef};
pub use filter::{Condition, Filter, Op};
pub use fs_watcher::{
    open_close_pairs, path_to_uri, to_did_change_watched_files, DidChangeWatchedFiles, FileEvent,
    FileEventKind, FsWatcher, NativeWatcher, OpenClosePair, WatchedFileChange,
};
pub use hash::FileHash;
pub use hybrid::{cosine, mmr_rerank, rrf_fuse, rrf_fuse_weighted, MmrCandidate, RRF_K};
pub use incremental::{IncrementalReport, ReindexReason};
pub use indexer::IndexOptions;
pub use lsp_surface::{cache_lsp_surface, CachedSurface, LspSurfaceRow, RefBloom, SurfaceDef};
pub use minhash::{
    ast_leaf_tokens, find_near_duplicates, jaccard_estimate, jaccard_exact, lsh_bands, minhash,
    minhash_source, trigrams, LshIndex, MinHashSig, NearDuplicate, CLONE_JACCARD, MINHASH_K,
};
pub use repair::{auto_repair, BrokenHunk, Fixed, NoopRepairModel, RepairModel, RepairRequest};
pub use search::{ChannelRanks, Embedder, NoopEmbedder, SearchHit, SearchOptions};
pub use signals::{symbol_signals, SignalVector, SymbolGraph};
pub use store::GraphStore;
pub use strictness::AstMatchStrictness;
pub use surface::{body_hash, surface_hash, surface_hash_file, FileSurface, SurfaceSymbol};
pub use tokenize::{fts_match_expr, split_for_fts, split_ident, tokenize};
pub use trace::{CrossServiceEdge, Direction, TraceOptions, TraceResult, TraceStep};
pub use tri_modal::{tri_modal, tri_modal_search, TriModalHit, TriModalOptions};
pub use types::{
    CallArg, CallCandidate, CallStrategy, Edge, EdgeType, FileChange, IndexReport, Label, Node,
};

// ── Phase 6-A re-exports ────────────────────────────────────────────────
pub use adr::{Adr, AdrQuery, AdrStatus, AdrStore};
pub use ann_repair::{repair_ann, RepairStats};
pub use bounded_rag::{bounded_select, Candidate, RagBudget};
pub use coherence::{coherence, CoherenceCompactor, CoherencePolicy, Entry};
pub use conformal::ConformalPredictor;
pub use lineage::Lineage;
pub use pdx::{PdxError, PdxLayout, PDX_HEADER_LEN, PDX_MAGIC};
pub use proof_gate::{GateDecision, ProofGate, RejectReason};
pub use quality::{DegradationFlag, QualityEnvelope, QualityPolicy};
pub use query_cache::{normalize_query, CachedResult, QueryCache};
pub use self_reconstruct::{checksum_of, ChecksumKind, Part, SelfReconstructingIndex};
pub use snapcompact::SnapCompact;
pub use sparsifier::{sparsify, Graph, WeightedEdge};
pub use tarl::{LedgerStats, Outcome, TarlLedger, TarlRecord};
pub use temporal::{decay, TemporalDecay, Weighted};
pub use two_random::TwoRandomCache;
pub use witness::{ReceiptChain, WitnessReceipt};
pub use witness_cache::WitnessKeyedCache;

// ── Phase 8 re-exports ──────────────────────────────────────────────────
pub use cypher::{
    CypherEngine, CypherError, CypherResult, CypherValue, Optimization, QueryPlan, Row, Rows,
};
pub use mcp_brain::{BrainEntry, BrainHit, BrainPartition, DriftFinding, SharedBrain};
pub use observation_masking::{
    truncate_result, Content, MaskedMessages, Message, ObservationMasker, Role,
};
pub use prometheus_metrics::{
    Counter, CounterHandle, Gauge, GaugeHandle, Health, HealthStatus, Histogram, HistogramHandle,
    MetricKind, MetricsRegistry, Readiness, DEFAULT_BUCKETS,
};
pub use scoped_context::{
    ContextHit, ContextIndexError, ContextIndexResult, QuarantineMark, RecoveryReport,
    ScopedContextIndex, Shard,
};

// ── Phase 6-D re-exports ────────────────────────────────────────────────
pub use capgated::{CapGatedIndex, CapMask, NodeId};
pub use cognitive_container::{CognitiveContainer, Witness};
pub use collections::{CollectionStats, Collections, CollectionsError};
pub use homology::{betti0_curve, bottleneck, h0_persistence};
pub use ns_routing::{NsRouter, ShardId};
pub use schema_cache::{
    canonicalize, default_compiler, resource_id, Compiled, ResourceId, SchemaCache,
};
pub use voi::{Costs, Incident, Ladder, Outcome as VoiOutcome, Policy, Stage};

// ── Phase 6-C re-exports (ANN algorithms) ───────────────────────────────
pub use acorn::AcornIndex;
// ── Phase 6-E: cut / condensation / decompile / diverse beam / SPANN ────
pub mod beam;
pub mod condense;
pub mod decompiler;
pub mod mincut;
pub mod spann;

pub use bounded::{bounded_search, HOP_DECAY};
pub use drifting::{jaccard_distance, DriftMonitor};
pub use gnn_rerank::{edge_nodes, gnn_rerank};
pub use lsm_bloom::{LsmIndex, Tier};
pub use speculative::{speculative_search, speculative_search_with_factor, OVER_FETCH};
pub use targeted::RecallTargeter;

// ── Phase 6-E re-exports ────────────────────────────────────────────────
pub use beam::diverse_beam;
pub use condense::{condense, CondensedGraph, SuperEdge, SuperNode};
pub use decompiler::{decompile, Community, Decomposition, Role as SymbolRole, WitnessNode};
pub use mincut::{
    certify_cut, community_map, label_propagation, stoer_wagner, CommunityId, Cut, CutWitness,
};
pub use spann::{spann_build, spann_search, ClusterId, Partitions, PointId};
