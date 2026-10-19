# Полный инвентарь ruvector: фишки для форка prime-agent (allspark)

Источник: `/root/vendor-source/ruvector` (read-only, сборки/тесты НЕ запускались).
Метод: локальный `rg`/`find`/чтение `src/lib.rs` + док-комментарии крейтов + `docs/adr/*.md`.
Метрики LOC/unsafe получены подсчётом `*.rs` под `crates/<name>/src` (+ подкаталоги, исключая `target/`, `node_modules/`).
`unsafe` = число `unsafe {`/`unsafe fn|impl|trait|extern` блоков в исходниках крейта (не только lib.rs).

Ограничение контекста: MCP `codebase-memory-mcp` в этой сессии недоступен (только `.codebase-memory/graph.db.zst` 148 МБ на диске); всё ниже — из локального чтения.

**Критичное для переноса**: `prime-agent/Cargo.toml:49` = `unsafe_code = "forbid"` (workspace lint), плюс `pedantic = "warn"` и CI `-D warnings`. Крейт с `unsafe` **не соберётся** без переписывания. В таблице колонка `unsafe` — по факту исходников, колонка `lint` — что объявлено в крейте.

---

## 1. Таблица релевантных крейтов

Формат: `LOC | unsafe(кол-во блоков) | lint-декларация | license`. Применимость: **pa-lsp** (клиент/символы/diagnostics), **pa-graph** (SQLite-граф, BM25+RRF, инкрементальная переиндексация), **edit** (нативные/AST-правки).

### 1a. Ядро: код-интеллект, граф, поиск, инкрементальность

| Крейт | LOC | Назначение | Ключевые типы | unsafe | License | Применимость |
|---|---:|---|---|---|---|---|
| `ruvector-graph` | 25 298 | Граф-БД (property graph+hypergraph+Cypher), **BM25, RRF, оптимизаторы/индексы** | `Bm25Index` (`bm25.rs:1`), `reciprocal_rank_fusion` (`schema.rs:501`), `TypedGraph::hybrid_search_text` (`typed_graph.rs:405`), `BloomFilter`, `AdaptiveRadixTree`, `RoaringBitmapIndex`, `QueryCache` (LRU+TTL), `Statistics`/`Histogram`, `QueryOptimizer` (predicate pushdown) | **14** (в `optimization/*`: `ptr::read` `adaptive_radix.rs:89`, `memory_pool.rs`, AVX2 `simd_traversal.rs`) | MIT (ws) | **pa-graph** ★★★, edit (Cypher-парсер — нет) |
| `ruvector-mincut` | 55 917 | Динамический min-cut (n^o(1)), **community/partition, witness trees, sparsify, expander, link-cut** | `GraphPartitioner::partition` (`integration/mod.rs:246`), `CommunityDetector::detect` (`integration/mod.rs:171`, рекурсивный min-cut), `WitnessTree` (`witness/mod.rs:1`), `DynamicMinCut`, `CactusGraph` (`canonical/mod.rs:172`) | **22** (все в `wasm/*` FFI и `optimization/simd_distance.rs` под `#[cfg(target_arch="wasm32")]`; ядро под `cfg_attr(not(wasm), deny(unsafe_code))` `lib.rs:119`) | MIT (ws) | **pa-graph** ★★★ (модульность/сообщества), edit (нет) |
| `ruvector-graph-condense` | 4 299 | Structure-preserving **конденсация графа** через min-cut; provenance сохраняется | `condense()`, `CondensedNode{members}` (`node.rs:9,24`), `StreamingCondenser` (lazy re-condense), `CondensationMetrics::cut_inflation` | **0** | MIT (ws), `#![forbid(unsafe_code)]` `lib.rs:77` | **pa-graph** ★★ (сжатие подграфа символов), ниша |
| `ruvector-sparsifier` | 2 754 | Динамическая **спектральная разрежённость** + аудит | `AdaptiveGeoSpar`, `SpectralAuditor::audit_quadratic_form`, `Backbone` (union-find spanning forest), `SpectralSampler` | **0** | MIT (ws) | pa-graph ★ (сжатие рёбер графа вызовов) |
| `ruvector-dag` | 15 527 | DAG-структуры + топологическая сортировка, attention, self-healing | `QueryDag` (`dag/query_dag.rs:22`), `topological_sort`/`ancestors`/`descendants` (`:254,279,304`), `TopologicalIterator`/`Dfs`/`Bfs` (`dag/traversal.rs`), `AttentionCache` (LRU+TTL `attention/cache.rs:1`) | **0** | MIT (ws) | **pa-graph** ★★★ (closure/topo, критический путь), edit (порядок правок) |
| `ruvector-delta-core` | 3 198 | **Delta-примитивы**: encode/compress/stream/window (event sourcing) | `DeltaStream` (checkpoint/replay/compact `stream.rs:1`), `SparseDelta`/`RunLengthEncoding`/`HybridEncoding`, `DeltaCompressor` (LZ4/Zstd/delta-of-delta `compression.rs:36`), `DeltaWindow` (tumbling/sliding/EMA) | 0 | MIT OR Apache-2.0 | **pa-graph** ★★★ (инкрементальная переиндексация), edit ★★ (патчи) |
| `ruvector-delta-graph` | 1 572 | Delta-операции над **графом** (edges/node props), delta-aware traversal | `GraphDelta{node_adds,edge_adds,...}`, `DeltaAwareTraversal` (BFS/DFS/Dijkstra `traversal.rs:11`), `affected_nodes()` (`lib.rs:227`) | 0 | MIT OR Apache-2.0 | **pa-graph** ★★★ (closure-repair), edit ★★ |
| `ruvector-delta-index` | 1 641 | **Инкрементальный HNSW** + repair-стратегии + quality monitor | `DeltaHnsw`, `IncrementalUpdater` (`incremental.rs`), `GraphRepairer`/`RepairStrategy{Lazy,Eager,Batched,Adaptive}` (`repair.rs`), `QualityMonitor`/`estimate_recall` (`quality.rs`) | 0 | MIT OR Apache-2.0 | **pa-graph** ★★★ (паттерн инкрементального ремонта индекса) |
| `ruvector-hnsw-repair` | 1 226 | HNSW deletion-стратегии: tombstone/batch/eager | `DeletionStrategy`, `TombstoneOnly`, `BatchRepair::flush`, `EagerRepair` (`strategy.rs:7`) | 0 | MIT (ws) | pa-graph ★ (удаление узлов) |
| `ruvector-hybrid` | 1 173 | **BM25 + dense + RRF/RSF/ScoreFusion** | `Bm25Index`, `FlatDenseIndex`, `RrfHybridIndex`, `ScoreFusionIndex`, `RsfHybridIndex` (`fusion.rs:1`) | **0** | MIT OR Apache-2.0, `forbid` `lib.rs:18` | **pa-graph** ★★★ (готовый BM25+RRF) |
| `ruvector-query-cache` | 1 128 | **Семантический кэш запросов** (cosine-близость запроса) | `SemanticCache::hit_threshold` (`semantic_cache.rs:1`), `ExactCache`, `CacheDecision` | 0 | MIT (ws) | **pa-graph/pa-lsp** ★★★ (кэш результатов поиска) |
| `ruvector-gnn-rerank` | 1 417 | GNN-score-diffusion **реранк кандидатов** ANN | `GnnDiffusionReranker`, `GnnMincutReranker`, `CandidateGraph::build` (`graph.rs:18`) | **0** | MIT (ws), `forbid` `lib.rs:26` | pa-graph ★★ (реранк выдачи) |

### 1b. Изоляция / namespace / scope

| Крейт | LOC | Назначение | Ключевые типы | unsafe | License | Применимость |
|---|---:|---|---|---|---|---|
| `ruvector-context` | 2 714 | **Физически изолированные** scope-шарды (per-scope VectorDB), fail-closed recovery | `ScopedContextIndex` (`index.rs`), `ContextNamespace{authority,tenant,subject_kind,subject_id,collection}` + `ContextScope` prefix-match (`scope.rs:22,107`), `QuarantinedShard` (`quarantine.rs`), `Shard::ann_search` — единственный путь к ANN + счётчик | **0** | MIT (ws), `forbid` `lib.rs:61`, `missing_docs=deny` | **pa-lsp/pa-graph** ★★★ (изоляция по проекту/воркспейсу, карантин битых индексов) |
| `ruvector-namespace-merge` | 1 220 | **S-T min-cut routing** между namespace памяти | `NamespaceRouter` trait; `AllSearch`/`CentroidFilter`/`MinCutRoute` (`router.rs`); `FlowGraph::max_flow`/`source_side` (Edmonds-Karp `flow.rs:1`) | 0 | MIT (ws) | pa-graph ★★ (какие namespace-шарды искать) |
| `ruvector-capgated` | 1 274 | **Capability-gated ANN**: 64-бит маска прав внутри поиска | `CapMask(u64)::satisfies` (`lib.rs:44`), `CapGatedIndex`, `CapGraph` (graph-walk prune) | 0 | MIT (ws) | pa-lsp/pa-graph ★★ (права/scope на выдачу) |
| `ruvector-collections` | 2 582 | Мульти-коллекции + алиасы; общая утилита Miller-Rabin | `CollectionManager`, alias-менеджер, `primality.rs` | 0 | MIT (ws) | pa-graph ★ (namespace-алиасы) |

### 1c. Аудит / provenance / witness / verification

| Крейт | LOC | Назначение | Ключевые типы | unsafe | License | Применимость |
|---|---:|---|---|---|---|---|
| `ruvector-proof-gate` | 1 268 | **Proof-gated записи** + MMR witness-лог | `WriteGate` trait; `NullGate`/`HashChainGate`/`MerkleGate` (MMR) (`gate.rs`); `WriteReceipt` (chain_commitment) | 0 | MIT OR Apache-2.0 | **pa-graph/edit** ★★★ (receipt на каждую мутацию индекса/файла) |
| `ruvector-retrieval-receipt` | 1 054 | **Receipt на выдачу поиска** (read-path) | `RetrievalReceipt::{PerResult,Merkle}`, `verify_full`, `index_state_root` (`receipt.rs`) | 0 | MIT OR Apache-2.0 | **pa-graph/pa-lsp** ★★★ (доказуемая выдача: файл:строка → receipt) |
| `ruvector-staged-workspace` | 1 667 | **Content-hash + revision binding**, fail-closed staleness | `WorkspaceState::commit`, `WorkspaceError::{StaleView,BindingMismatch,AnchorRejected}` (`workspace.rs:1`), `StagedView`, `TransitionAnchor` (`anchor.rs`) | 0 | MIT OR Apache-2.0 | **edit/pa-lsp** ★★★ (запрет правок по устаревшему read) |
| `ruvector-verified` | 2 317 | Proof-carrying vector ops (lean-agentic), дедуп термов | `ConversionCache`, `ProofEnvironment`, `ProofAttestation`, `FastTermArena` (bump+dedup 4-wide) | 0 | MIT OR Apache-2.0 | pa-graph ★ (идея дедуп-кэша термов) |
| `rvforge-registry` | 6 042 | Content-addressed registry: trust ladder, revocation, **Merkle transparency log** | `objects/{publisher,release,revocation,transparency,trust,witness}.rs` | **0** | MIT (ws), `forbid` `lib.rs:74` | ★ (модель trust для плагинов/skills) |
| `rvf-forge-core` | 5 307 | Пакетирование/verify канонического `.rvf`; canonical-JSON build manifest | `ContainerBuilder`, `verify`, `provenance`, `checksums` (`lib.rs:1`) | **0** | MIT (ws), `forbid` `lib.rs:98` | ★ (provenance артефактов) |
| `ruvector-solver` | 14 574 | Итеративные sparse-солверы + **budget enforcement + audit trail** | `BudgetEnforcer::check_iteration/check_memory` (`budget.rs:1`), `SolverAuditEntry` (`audit.rs`), `SolverArena` (bump `arena.rs`), `SolverRouter` (выбор алгоритма) | **24** (`types.rs:113` unsafe CSR-индексация) | MIT (ws) | pa-graph ★★ (бюджеты/лимиты на вычисления) |

### 1d. Кэш / компрессия / дедуп / tiering

| Крейт | LOC | Назначение | Ключевые типы | unsafe | License | Применимость |
|---|---:|---|---|---|---|---|
| `ruvector-core` | 32 674 | VectorDB+**MMR, LSM-compaction, sparse+RRF, PDX-раскладка, арены, lock-free, conformal** | `MMRSearch` (`advanced_features/mmr.rs:4`), `LSMIndex`/`MemTable`/`Segment`+`BloomFilter` (`compaction.rs:1`), `SparseIndex`+`fuse_rankings` (`sparse_vector.rs:1`), `PdxBlock` (`pdx.rs:1`), `Arena`/`CacheAlignedVec` (`arena.rs`), `LockFreeCounter/ObjectPool/WorkQueue` (`lockfree.rs`), `ConformalPredictor` (`conformal_prediction.rs:1`) | **95** (SIMD NEON/AVX2 в `quantization.rs:79,343,433`; `libc` в context) | MIT (ws) | **pa-graph** ★★★ (много переиспользуемых примитивов) |
| `ruvector-temporal-tensor` | 15 501 | **Tiered quantization** 8/7/5/3 бит + temporal segment reuse + delta-chains | `TemporalTensorCompressor`, `TierPolicy::select_bits` (`tier_policy.rs:1`), `tiering::compute_score/choose_tier` (EMA+popcount+recency+hysteresis `tiering.rs:7`), `DeltaChain`, `SparseDelta` (`delta.rs:1`) | **23** | MIT (ws) | pa-graph ★★ (tiering горячее/холодное для индекса) |
| `ruvector-lsm-ann` | 1 209 | **LSM для векторов**: mutable L0 + L1/L2 compaction | `LsmIndex` trait, `LsmConfig`, `merge_candidates`, `brute_force_knn` | 0 | MIT (ws) | pa-graph ★★ (write-heavy ингест) |
| `ruvector-rulake` | 5 237 | Федерация бэкендов + **witness-addressed RaBitQ-кэш** | `VectorCache` keyed by SHAKE-256 witness (`cache.rs:1`), `invalidate`, refcount GC, `Consistency` | 0 | MIT (ws) | **pa-graph** ★★★ (кэш, ключ = content-hash, авто-инвалидация) |
| `ruvector-matryoshka` | 1 151 | Coarse-to-fine поиск по усечённому вектору | `TwoStageIndex`/`ThreeStageIndex`, `prefix_project` | 0 | MIT (ws) | pa-graph ★★ (сначала дешёвое сравнение) |
| `ruvector-pq-search` | 1 187 | Product Quantization + ADC | `FlatPqIndex`/`IvfPqIndex`/`ResidualPqIndex` | 0 | MIT (ws) | pa-graph ★ |
| `ruvector-spann` | 1 089 | Partition spilling (boundary-safe ANN) | `SinglePartition`/`SpillPartition`/`CoherenceSpill` | 0 | MIT OR Apache-2.0 | pa-graph ★ |
| `ruvector-rairs` | 1 415 | IVF redundant assignment + dedup blocks | `RairsSeil` (shared 32-vec blocks) | **0** | MIT OR Apache-2.0, `forbid` `lib.rs:27` | pa-graph ★ |
| `ruvector-diskann` | 4 055 | DiskANN/Vamana + **fixed-topology reuse под drift** | `VamanaGraph`, `DriftingIndex`, `RebuildPolicy`, `RecallTrigger` (`reuse.rs:1`) | **12** | MIT (ws) | pa-graph ★★ (не перестраивать граф целиком при дрейфе) |
| `ruvector-snapshot` | 849 | Снапшоты с compression+checksums | `SnapshotManager`, `SnapshotStorage`, `LocalStorage` | 0 | MIT (ws) | pa-graph ★ |
| `ruvector-coherence` | 1 347 | Метрики когерентности + **спектральный health-monitor** | `jaccard_similarity`, `SpectralTracker`, `HnswHealthMonitor`, `estimate_fiedler` (`spectral.rs:310`) | **4** (NEON `spectral.rs:56,80`) | MIT (ws) | pa-graph ★★ (детект деградации индекса) |
| `ruvector-streaming-qng` | 1 549 | Streaming PQ с обновлением codebook при дрейфе | `StreamPQ` (reservoir sample) | 0 | MIT (ws) | pa-graph ★ |

### 1e. Стриминг / батчинг / параллелизм / метрики

| Крейт | LOC | Назначение | Ключевые типы | unsafe | License | Применимость |
|---|---:|---|---|---|---|---|
| `ruvector-metrics` | 497 | Prometheus-метрики + health/readiness | `MetricsRecorder`, `HealthChecker` | 0 | MIT (ws) | ★★ (телеметрия pa-graph) |
| `ruvector-profiler` | 679 | Память/энергия/латентность + CSV | `LatencyStats`, `MemoryTracker`, `PowerTracker`, `config_hash` | 0 | MIT (ws) | ★★ (профиль индексации) |
| `ruvector-bench` | 3 649 | ANN-Benchmarks-совместимый харнесс, p50..p99.9 | `LatencyStats`, `DatasetGenerator`, `ResultWriter` | 0 | MIT (ws) | ★★ (бенчмарки pa-graph) |
| `ruvector-sota-bench` | 2 891 | SOTA-протокол (ADR-265/267), score-функция | `score = 0.40·recall + ...` | 1 | MIT (ws) | ★ (валидация) |
| `ruvector-cluster` | 2 888 | Distributed: consistent-hash sharding, DAG-consensus, gossip | `ConsistentHashRing`, `ShardRouter`, `DagConsensus`, `GossipDiscovery` | 0 | MIT (ws) | ★ (не для локального форка) |
| `ruvector-raft` | 3 150 | Raft consensus | `RaftNode`, `LeaderState` | 0 | MIT (ws) | none |
| `ruvector-replication` | 2 107 | Репликация, failover, CDC | `ReplicaSet`, `SyncManager` | 0 | MIT (ws) | none |
| `ruvector-delta-consensus` | 1 641 | CRDT delta-merge + vector clocks | `VectorClock`, `DeltaConsensus`, `DeltaGossip` | 0 | MIT OR Apache-2.0 | ★ (merge локальных индексов) |

### 1f. Код-интеллект / декомпиляция / AST (ближайшее к pa-lsp/edit)

| Крейт | LOC | Назначение | Ключевые типы | unsafe | License | Применимость |
|---|---:|---|---|---|---|---|
| `ruvector-decompiler` | 7 495 | **JS-декомпилятор**: parse→ref-graph→mincut/louvain→infer names→witness; **иерархия «папки = структура графа»** | `decompile()` (5 фаз `lib.rs:1`), `ReferenceGraph`, `partition_modules` (exact MinCut <5K, Louvain ≥5K `partitioner.rs:1`), `build_module_tree` (`tree.rs:3`), `WitnessChain`, V3 source-map VLQ | **3** (`from_utf8_unchecked` `parser.rs:246,281,292`) | MIT (ws) | **pa-graph** ★★★ (структурная кластеризация символов), edit ★★ (модульный layout) |
| `ruvector-decompiler-wasm` | 50 | WASM-обёртка | `decompile(source, config_json)` | 0 | MIT | ★ |
| `ruvector-gnn` | 10 082 | GNN-слои + **анти-забывание** (Adam/Replay/EWC/scheduler) | `ReplayBuffer`, `ElasticWeightConsolidation`, `LearningRateScheduler` | 12 | MIT (ws) | ★ |
| `ruvector-math` | 13 826 | TDA/персистентная гомология, тропическая алгебра, тензорные сети | `homology/*` (Betti, bottleneck/Wasserstein), `MinPlusMatrix::closure` (`tropical/matrix.rs:114`) | 0 | MIT (ws) | ★ (топологический drift-детект) |
| `ruvector-attention` | 20 443 | Attention (graph/hyperbolic/sparse/MLA/Mamba) | `MLACache` (KV-компрессия) | 0 | MIT (ws) | none |
| `ruvector-acorn` | 1 088 | Predicate-agnostic filtered HNSW (ACORN) | `AcornIndex1`/`AcornIndexGamma` | 0 | MIT (ws) | **pa-graph** ★★ (фильтр по типу/файлу без collapse recall) |
| `ruvector-maxsim` | 1 974 | ColBERT MaxSim late-interaction | `FlatMaxSim`/`BucketMaxSim`/`HnswMaxSim` | **0** | MIT (ws), `forbid` `lib.rs:39` | ★★ (multi-vector на символ) |
| `ruvector-temporal-coherence` | 1 108 | Temporal decay + coherence-gate | `TemporalSearch`/`CoherenceSearch`, `CoherenceGraph` | 0 | MIT OR Apache-2.0 | ★★ |
| `ruvector-agent-memory` | 6 135 | Compaction (CoherencePolicy) + **TARL-ledger** + CAMA-arbitration + causal episodic graph | `CoherencePolicy` (`compaction.rs`), `TransactionalLedger` (5 операций `ledger.rs:1`), `arbitrate` (`arbitration.rs:2`), `AtomicObservation`/`CausalEpisodicGraph` (`observation.rs`,`fusion.rs`) | 0 | MIT OR Apache-2.0 | **pa-graph** ★★★ (ledger приёмки/карантина памяти) |

### 1g. Harness-паттерны (rvAgent, mcp-gate, tiny-dancer)

| Крейт | LOC | Назначение | Ключевые типы | unsafe | License | Применимость |
|---|---:|---|---|---|---|---|
| `rvAgent` (rvagent-core) | 69 867 | Agent harness: **observation masking**, string-pool, subagent | `MaskConfig`/`mask_observations`/`truncate_tool_result` (`masking.rs:1`), `StringPool` (`string_pool.rs:1`), `Arena` | 4 | MIT OR Apache-2.0 (per-crate) | ★★★ (контекст/бюджеты) |
| `mcp-gate` | 4 690 | MCP-gate: **VoI escalation ladder** + **ReCache schema-cache** | `EscalationLadder::inspect` (`monitor/mod.rs:1`), `SchemaResourceCache::assemble` (`schema_cache.rs:1`), `ResourceId = SHA-256(canonical JSON)` | 0 | MIT OR Apache-2.0 | **edit/pa-lsp** ★★★ (кэш схем тулов, VoI) |
| `ruvector-tiny-dancer-core` | 6 506 | Neural routing + **VoI primitive** + conformal + circuit-breaker | `voi::decide` (Pandora `voi.rs:3`), `UncertaintyEstimator`, `CircuitBreaker`, `FastGRNN` | 0 | MIT (ws) | ★★ (VoI: когда тратить дорогой вызов) |

### 1h. Прочее, найденное в этом проходе (не в исходном списке)

| Крейт | LOC | Назначение | unsafe | License | Применимость |
|---|---:|---|---|---|---|
| `ruvector-filter` | 1 510 | Payload-индексы (int/float/keyword/bool/geo/text) + выражение-фильтр | 0 | MIT (ws) | **pa-graph** ★★ (фильтр по атрибутам символов) |
| `ruvector-bounded-rag` | 1 068 | RAG под token-budget: `TopK`/`GraphBfs`/`MinCutBounded` | 0 | MIT (ws) | **pa-lsp/pa-graph** ★★★ (бюджет контекста) |
| `ruvector-cluster-rag` | 1 127 | RAPTOR-подобное cluster-summary retrieval | 0 | MIT (ws) | ★★ |
| `ruvector-diverse-beam` | 1 104 | MMR post-rerank + coherence-pruned beam | 0 | MIT (ws) | ★★ |
| `ruvector-speculative-ann` | 1 400 | Draft u8 → verify f32 (speculative decoding) | 0 | MIT (ws) | ★★ |
| `ruvector-adaptive-ann` | 1 374 | Recall-targeted поиск (авто-ef) | 0 | MIT (ws) | ★★ |
| `ruvector-entropy-ann` | 1 407 | Entropy-adaptive beam (**ADR-303 — отрицательный результат**) | 0 | MIT OR Apache-2.0 | ★ (анти-паттерн) |
| `ruvector-rabitq` | 3 847 | RaBitQ 1-bit, HadamardSigned rotation O(D log D) | **19** (`scan.rs:71-99` `get_unchecked`) | MIT (ws) | **pa-graph** ★★ (компрессия, но unsafe) |
| `ruvector-turbovec` | 1 535 | TurboQuant multi-bit FastScan (2/3/4-bit) | 0 | MIT (ws) | ★★ (multi-bit, safe!) |
| `ruvector-turboquant` | 1 865 | Turbo4 4-bit datatype, Hadamard rotation без rand | 15 | MIT (ws) | ★ |
| `ruvector-attn-mincut` | 735 | Min-cut gating + **witness SHA-256 determinism** | 0 | MIT (ws) | ★ |
| `ruvector-coherence-hnsw` | 1 363 | Coherence-gated beam search | 0 | MIT (ws) | ★ |
| `prime-radiant` | 69 483 | Sheaf-Laplacian coherence engine, compute-ladder escalation | **0** | MIT OR Apache-2.0, `deny(unsafe_code)` `lib.rs:147` | ★★ (идея «escalate-only») |
| `rvf` (rvf-types/rvf-wire) | 80 405 | Канонический формат: LEB128 varint, delta+restart points, **QualityEnvelope** | 122 | (исключён из ws) | **pa-graph** ★★ (quality/degradation reporting) |
| `sona` | 12 794 | Micro-LoRA + ReasoningBank (K-means++ паттерны) + EWC++ | 1 | MIT OR Apache-2.0 | ★ |
| `ruvector-timesfm` | 1 229 | Прогноз временных рядов + anomaly-band (телеметрия) | 3 | MIT (ws) | ★ |
| `ruvector-cognitive-container` | 1 507 | WASM-контейнер с witness-цепочкой по эпохам | 0 | MIT (ws) | ★ |
| `ruvector-perception` | 3 881 | delta→boundary→coherence→proof→action (evidence-gated) | **0** | MIT (ws), `forbid` `lib.rs:58` | ★★ (модель «доказательство перед действием») |

**Итого**: 32 крейта из 177 содержат `unsafe`-блоки; 139 — чисто safe. Всё, что ниже в Топ-10, кроме явно отмеченного, — `unsafe`-free.

---

## 2. НОВЫЕ находки (чего не было в исходном списке известного)

### N1. `ruvector-decompiler` — «структура папок = структура графа» ⭐⭐⭐
Самое релевантное для pa-graph из непокрытого. Полный конвейер: `parse → ReferenceGraph → partition → infer → witness` (`lib.rs:1`). Ключевые приёмы:
- **Адаптивный выбор алгоритма**: exact MinCut (`GraphPartitioner`) при `<5000` узлов, Louvain `O(n log n)` (параллельный через rayon) при `≥5000` — `partitioner.rs:3`. Прямо переносится на «граф символов кода».
- **Иерархия**: `build_module_tree` — inter-module edge density → агломеративная кластеризация → рекурсивный split до `max_depth`; «folder names inferred from discriminative string context» (`tree.rs:3`).
- **Имя папки/символа** выводится из discriminative string context + property correlation (`inferrer.rs`), с confidence и обучением на corrections (`InferenceFeedback::learn_from_ground_truth`).
- **Witness chain** (Merkle) доказывает, что вывод происходит из входа (`witness.rs:1`) + V3 source-map с VLQ (`sourcemap.rs:1`).
Опасность: 3× `from_utf8_unchecked` в `parser.rs:246,281,292` — тривиально заменяются на `from_utf8(...)?`.

### N2. `ruvector-context` — физическая изоляция scope-шардов ⭐⭐⭐
Не «метаданные-фильтр после ANN», а **отдельный VectorDB на каждый exact scope** (`ADR-334`). Инварианты: поиск в namespace A никогда не вызывает ANN-индекс namespace B; filename = SHA-256(domain-separated, length-delimited encoding), tenant/path не попадают в ФС; recovery проверяет, что манифест хэшируется в имя файла, иначе **fail-closed**; битые шарды уходят в `QuarantinedShard` (не блокируют весь индекс, но видны и восстановимы `quarantine.rs:1`). Плюс счётчик `searches` на шард — «траверсал без подсчёта не компилируется» (`shard.rs:24`). Для pa-lsp/pa-graph: изоляция по (проект, воркспейс, тип коллекции: memory/resources/skills).

### N3. `ruvector-rulake` — кэш, адресуемый witness-хэшем ⭐⭐⭐
`VectorCache` ключуется не по `(backend, collection)`, а по SHAKE-256 witness: два бэкенда с одинаковыми `(data_ref, rotation_seed, rerank_factor, generation)` дают одинаковый witness → делят одну запись. Вторичный pointer-map резолвит `(backend,collection)` → witness; при смене generation указатель переезжает, старая запись GC-ится по refcount (`cache.rs:1`). Это ровно лекарство от «cache-invalidation drift» для индекса кода: ключ кэша = content-hash состояния репозитория.

### N4. `ruvector-staged-workspace` — структурная защита от правок по устаревшему чтению ⭐⭐⭐
ADR-318: view привязан к `(content_hash, revision_id)`; при несовпадении — **hard reject**, не warning. `WorkspaceError::StaleView` (hash изменился) и `BindingMismatch` (hash совпал, revision нет — подделка) (`workspace.rs:1`). Коммит идентичных байт = no-op (без bump revision → ни одна view не устаревает). «No anchor, no transition»: запись идёт через `TransitionAnchor` **до** мутации (`anchor.rs:1`). Для edit-слоя: инструмент `edit` обязан нести hash/revision от `read`; иначе структурный отказ. Есть готовый тест-инвариант `tests/invariant.rs:74 mutation_invalidates_all_five_view_kinds`.

### N5. `mcp-gate` — ReCache (кэш схем) + VoI-лестница ⭐⭐⭐
- **`SchemaResourceCache`**: `ResourceId = SHA-256(canonical JSON, keys sorted recursively)` (`schema_cache.rs:1`); блок компилируется **один раз на контент**, контекст собирается конкатенацией кэшированных блоков → смена порядка тулов не перекомпилирует всё. Прямо применимо к системному промпту/описаниям инструментов prime-agent.
- **`EscalationLadder`** (ADR-337): `stream → дешёвый детектор → risk → local verifier → strong investigator → halt`; правило «investigate when `P(violation)×damage > verification_cost`»; **unconditional-inspection floor** (privilege escalation, network, credentials, destructive ops инспектируются всегда, независимо от экономики) (`monitor/mod.rs:1`). Готовый паттерн для решений «тратить ли дорогой вызов/субагента».

### N6. `ruvector-tiny-dancer-core::voi` — примитив VoI с закрытой формой ⭐⭐
`decide` / `value_of_information` / `voi_upper_bound` при `U ~ N(μ,σ²)`: покупать оценку только при положительном ожидании (`voi.rs:3`). Рядом `UncertaintyEstimator` (conformal, калибровка квантиля) и `CircuitBreaker` (деградация). Применимо к «нужен ли глубокий поиск/LLM-реранк или хватит дешёвого BM25».

### N7. `ruvector-core::advanced_features` — набор готовых safe-примитивов ⭐⭐⭐
То, что не было в известном списке, но лежит в `ruvector-core`:
- **MMR** (Maximal Marginal Relevance) для diversity-aware rerank: `MMR = λ·sim(q,d) − (1−λ)·max sim(d,selected)` (`mmr.rs:4`) — против выдачи 10 почти одинаковых символов.
- **LSM-компакция** (`compaction.rs:1`): `MemTable` (mutable L0) + immutable `Segment` L1/L2 + `BloomFilter` (negative lookup O(1)) + tombstones + `write_amplification` метрика.
- **Sparse + RRF fusion** (`sparse_vector.rs:1`): `SparseVector` (sorted idx+val), inverted posting lists, SPLADE-compatible scoring, `FusionStrategy{RRF,Linear,DBSF}`.
- **PDX-раскладка** (`pdx.rs:1`): dimension-major layout даёт ~2× на **скалярном** коде без интринсиков («layout is the lever, not intrinsics», SIGMOD'25) — применимо к батч-сканированию.
- **Arena** (`arena.rs`): cache-aligned bump-аллокатор + `ArenaVec` + thread-local arena; **LockFree** `ObjectPool`/`WorkQueue`/`Counter` (`lockfree.rs`).
- **Conformal prediction** (`conformal_prediction.rs:1`) и **фильтрованный поиск с авто-выбором pre/post-filter по селективности** (`filtered_search.rs:1`).
- **Learned index / neural hash / TDA** (`advanced/*`) — экспериментальные.

### N8. `ruvector-graph` — BM25+RRF+индексы и cost-based optimizer ⭐⭐⭐
`ruvector-graph` (25K LOC) содержит **самостоятельный** код-поисковый арсенал помимо Cypher:
- `bm25.rs:1` — компактный Okapi BM25 (инвертированный индекс, tie-break для RRF); `schema.rs:501` — `reciprocal_rank_fusion` (`Σ 1/(k+rank)`, k=60).
- `typed_graph.rs:405` — **tri-modal hybrid**: ANN-вектор + BM25 + graph-traversal, fusion RRF, затем обход из fused top-k; over-fetch `k·4` на каждую ветку.
- `index.rs` — `LabelIndex`/`PropertyIndex`/`EdgeTypeIndex`/`AdjacencyIndex`.
- `optimization/`: `BloomFilter` (Scalable/Counting), `AdaptiveRadixTree` (ART с path compression), `RoaringBitmapIndex` + `DeltaEncodedList` + dictionary encoding (`index_compression.rs`), `query_jit.rs` (hot-path «JIT»), `cache_hierarchy.rs` (hot/cold separation, prefetch), `memory_pool.rs`.
- `executor/cache.rs` — LRU+TTL query-cache; `executor/stats.rs` — histogram/MCV статистика и оценка селективности для cost-based планирования; `cypher/optimizer.rs` — predicate pushdown, join reordering, constant folding.
Опасность: unsafe сосредоточен в `optimization/` (ptr::read, memory_pool, AVX2 simd_traversal) — эти модули опциональны, ядро (bm25/schema/index/executor) safe.

### N9. `ruvector-mincut` — community detection + witness trees для графа кода ⭐⭐
Помимо собственно min-cut: `CommunityDetector::detect` — **рекурсивное** разбиение по min-cut (`integration/mod.rs:171,198`), `GraphPartitioner::partition` (`:246`), `WitnessTree` (Jin-Sun-Thorup SODA'24) — сертификация min-cut через spanning forest с «witness» на каждом tree-edge (`witness/mod.rs:1`). Применимо к разбиению графа кода на модули/кластеры с сертификатом качества разреза. Ядро safe при `not(wasm)`.

### N10. `ruvector-sparsifier` — разрежённость + самоаудит ⭐⭐
`Backbone` (spanning forest на union-find, гарантия связности), `EffectiveResistanceEstimator` (random-walk importance), `SpectralSampler` (keep с вероятностью `∝ w·R_eff·log n/ε²`), `SpectralAuditor` (случайные квадратичные пробы `xᵀLx` детектят дрейф → recommend rebuild). Готовый паттерн «сжали граф — проверь, что энергия Лапласиана сохранилась».

### N11. `ruvector-agent-memory` — TARL-ledger + CAMA + causal graph ⭐⭐⭐
Помимо известной `CoherencePolicy`:
- **`TransactionalLedger`** (TARL, ADR-307): 5 операций над памятью (`add / ignore / revise_outdated_belief / reject_unreliable / defer_for_verification`) с состояниями accepted/pending/rejected, полной provenance-историей и «stage → witness → commit» (нет witness — нет мутации) (`ledger.rs:1`). Это структурный ответ на memory-poisoning.
- **CAMA-arbitration** (`arbitration.rs:2`): N воспоминаний с общим `causal_parent` = 1 наблюдение, не N; evidence-lineage кластеризация, «downgrade-only confidence».
- **`AtomicObservation`** + `CausalEpisodicGraph` (ADR-320): content-addressed (id = SHA-256 канонических байт), tenant-isolated, обязательная верификация подписи и **резолвимость `causal_parents`** (unresolvable/cyclic parent → hard reject → отношение всегда DAG) (`observation.rs`,`fusion.rs`).
- **D²ACCI stage-diagnostic** (`diagnostic.rs:2`): локализация, какой из 4 стадий (ingestion/retrieval/filtering/generation) ответственен за регресс.

### N12. `ruvector-bounded-rag` + `namespace-merge` — бюджет и маршрутизация ⭐⭐
`bounded-rag`: три стратегии уважающие **token-budget** (`TopK` / `GraphBfs` с coherence-gate / `MinCutBounded` через max-flow) (`lib.rs:3`). `namespace-merge`: S-T min-cut routing (Edmonds-Karp, `flow.rs:1`) — «какие namespace искать», с baseline `AllSearch` и эвристикой `CentroidFilter` для сравнения.

### N13. `rvf` — QualityEnvelope и деградация ⭐⭐
`rvf-types/src/quality.rs:1`: обязательный внешний тип ответа с `RetrievalQuality` (Full/Partial/LayerAOnly/DegenerateDetected/BruteForceBudgeted), `ResponseQuality` (Verified/Usable/Degraded), `BudgetReport`, `DegradationReason`, `FallbackPath`. Плюс wire-примитивы: LEB128 varint (`varint.rs`), delta-encoding с restart points для random access (`delta.rs:1`), `LineageRecord` (parent→child provenance, `lineage.rs:1`). Паттерн «ответ всегда несёт, насколько он деградирован».

### N14. `rvAgent` — observation masking (ADR-274) ⭐⭐⭐
Замена старого tool-вывода на компактный placeholder с **dereferenceable recall-id** (`masking.rs:1`): `[read_file output elided: 2847 bytes, recall id tc7]`. `AgentState.messages` остаётся полным append-only логом (masking = проекция, не мутация); `truncate_tool_result` режет по границе UTF-8 с явным маркером; `keep_last_observations` (default 8), `max_tool_result_bytes` (default 100k). ADR-274 обосновывает: masking ≥ LLM-summarization по solve-rate при ~половинной стоимости. Плюс `StringPool` (DashMap-интернирование tool/field-имён).

### N15. Пробел: SCC / Tarjan / конденсация графа — НЕ реализованы
Поиск по `crates/` не нашёл ни Tarjan/Kosaraju SCC, ни condensation-графа SCC. Есть только:
- `ruvector-dag`: топологическая сортировка + `ancestors`/`descendants` + детект цикла при `add_edge` (`dag/query_dag.rs:254,279,304`; `DagError::CycleDetected`); `ruvector-dag-wasm/src/lib.rs:246` — reachability-проверка.
- `prime-radiant/src/governance/repository.rs:910` — **Kahn's algorithm** для детекта цикла (DAG-валидация ADR-графа зависимостей).
- «condensation» в ruvector = **graph condensation** (сжатие графа, `ruvector-graph-condense`), НЕ SCC-конденсация — термины совпадают, смысл разный.

**Следствие для pa-graph**: SCC (Tarjan/Kosaraju) и SCC-condensation придётся писать самим (или брать `petgraph`, которого нет в lock). Kahn-валидация DAG + топосорт переносимы из `ruvector-dag`/`prime-radiant`.

---

## 3. Пересмотр решений

1. **`ruvector-hybrid` — не единственный источник BM25+RRF.** BM25+RRF есть **трижды**: `ruvector-hybrid` (1173 LOC, forbid-unsafe), `ruvector-graph::bm25.rs`+`schema.rs:501` (в составе граф-БД, вместе с индексами/оптимизатором), `ruvector-core::advanced_features/{hybrid_search,sparse_vector}.rs` (плюс SPLADE-совместимый sparse и три fusion-стратегии). Для pa-graph, который и так строит SQLite-граф, разумнее брать BM25/RRF **вместе с граф-слоем** (`ruvector-graph`), а не отдельным крейтом.

2. **`ruvector-rabitq` (известен как «не соберётся»)** — подтверждено: 19 unsafe-блоков, все `get_unchecked` в `scan.rs:71-99`. Но появилась **safe-альтернатива**: `ruvector-turbovec` (1535 LOC, **0 unsafe**) — multi-bit (2/3/4-bit) FastScan-подобный индекс, переиспользует rotation из rabitq, добавляет `IdMapIndex` (O(1) удаление, allowlist-фильтр). Вывод меняется: для multi-bit брать `turbovec`; `rabitq` — только если нужен именно 1-bit и готовы переписать hot-loop.

3. **`ruvector-decompiler` меняет статус с «none» на приоритетный для pa-graph.** Ранее он не фигурировал. Это готовый Rust-конвейер «AST-подобный разбор → граф ссылок → min-cut/Louvain → иерархия → witness», то есть ровно то, что планируется в pa-graph для кластеризации символов и closure-repair. Дополнительно: он **зависит от `ruvector-mincut`** — то есть подтверждает переносимость min-cut в граф-задачу кода (ADR-135 «mincut-decompiler-with-witness-chains»).

4. **Инкрементальность: не один крейт, а семейство.** Помимо `delta-core`/`delta-graph`: `ruvector-delta-index` (инкрементальный HNSW + `RepairStrategy{Lazy,Eager,Batched,Adaptive}` + quality-monitor с оценкой recall и CI), `ruvector-graph-condense::StreamingCondenser` (**lazy re-condense** с `is_dirty` — честно заявлено как «не sublinear», `stream.rs:1`), `ruvector-diskann::DriftingIndex` (fixed-topology reuse под drift, rebuild по `RecallTrigger`), `ruvector-lsm-ann`, `ruvector-temporal-tensor::DeltaChain` (bounded-depth с авто-компакцией). Для pa-graph: «dirty-флаг + отложенный rebuild + триггер по деградации recall» — самый дешёвый и проверяемый паттерн.

5. **Изоляция: физические шарды важнее фильтров.** Прежний вывод «scope/namespace — это метаданные» пересматривается: `ruvector-context` (ADR-334) прямо утверждает, что post-filter после ANN **не является границей тенанта** — нужен отдельный индекс на scope + fail-closed recovery + карантин.

6. **Кэш: ключ должен быть content-hash, а не логическим ключом.** `rulake` (witness-keyed) + `staged-workspace` (content-hash+revision binding) + `mcp-gate::ReCache` (SHA-256 канонического JSON) дают согласованную тройку. Это меняет дизайн кэша pa-lsp/pa-graph: инвалидация — следствие смены hash, а не ручной вызов.

7. **Unsafe-картина шире, чем «rabitq + несколько».** 32 крейта из 177 содержат unsafe. Но: 7 крейтов явно `forbid(unsafe_code)`, `ruvector-mincut` — `deny` вне wasm, `prime-radiant` — `deny`. То есть значительная часть unsafe изолирована в wasm-FFI/SIMD-путях и не обязательна к переносу.

8. **ADR-303 (`entropy-ann`) — отрицательный результат.** Не тратить время на entropy-adaptive beam: в ADR зафиксировано, что мягкая энтропия отслеживает локальную плотность, а не неоднозначность, и saturates.

---

## 4. Топ-10 для переноса

| # | Источник | Что берём | Почему (выгода) | unsafe-статус |
|---|---|---|---|---|
| 1 | `ruvector-decompiler` (`partitioner.rs`,`tree.rs`,`graph.rs`,`witness.rs`) | Адаптивный `parse→ref-graph→min-cut/Louvain→иерархия` для графа символов | Готовый Rust-конвейер кластеризации кода; `<5K` exact, `≥5K` Louvain rayon | 3× `from_utf8_unchecked` → тривиально safe |
| 2 | `ruvector-hybrid` + `ruvector-graph::bm25/schema` | BM25 (k1=1.2,b=0.75) + **RRF** (`Σ1/(k+rank)`, k=60) + tri-modal hybrid | Заменяет/дополняет одиночный FTS5; RRF устойчив к разным шкалам BM25/cosine | **0 unsafe** (`forbid`) |
| 3 | `ruvector-staged-workspace` (`workspace.rs`,`anchor.rs`) | Content-hash+revision binding, fail-closed staleness, «no anchor no transition» | Структурно убивает класс ошибок «правка по устаревшему read» (ADR-273: патч-фейлы — главный источник потерь) | **0 unsafe** |
| 4 | `ruvector-rulake::cache` + `mcp-gate::schema_cache` | Кэш, ключ = witness/content-hash; `ResourceId=SHA-256(canonical JSON)` | Инвалидация как следствие смены hash; переиспользование схем тулов | **0 unsafe** |
| 5 | `ruvector-context` (`scope.rs`,`shard.rs`,`quarantine.rs`) | Per-scope физические шарды + fail-closed recovery + карантин | Настоящая изоляция проект/воркспейс вместо post-filter | **0 unsafe** (`forbid`, `missing_docs=deny`) |
| 6 | `ruvector-proof-gate` + `ruvector-retrieval-receipt` | HashChain/Merkle (MMR) gate на запись; receipt на выдачу поиска | Доказуемость provenance: «файл:строка пришёл из этого состояния индекса» | **0 unsafe** |
| 7 | `ruvector-delta-index` + `ruvector-graph-condense::StreamingCondenser` + `diskann::DriftingIndex` | Инкрементальный repair: `dirty`-флаг, `RepairStrategy`, триггер по recall | closure-repair без полного rebuild | **0 unsafe** |
| 8 | `ruvector-core::advanced_features/{mmr,compaction,sparse_vector}` + `pdx` | MMR-diversity rerank, LSM+Bloom, sparse+RRF, PDX-раскладка | Дешёвые выигрыши по качеству выдачи/памяти без интринсиков | **0 unsafe** в этих модулях |
| 9 | `rvAgent::masking` + `mcp-gate::monitor` (VoI) | Observation masking с recall-id; VoI-лестница «investigate when P×damage>cost» + mandatory floor | Резка контекста без LLM-суммаризации; гейт на дорогие вызовы | **0 unsafe** |
| 10 | `ruvector-agent-memory` (`ledger.rs`,`arbitration.rs`,`observation.rs`) | TARL-ledger приёмки (5 операций), CAMA-lineage, content-addressed observation | Структурная защита памяти индекса от poisoning/дублей | **0 unsafe** |

Резерв (если понадобится): `ruvector-gnn-rerank` (0 unsafe) — реранк кандидатов; `ruvector-turbovec` (0 unsafe) — multi-bit компрессия вместо unsafe-rabitq; `ruvector-filter` (0 unsafe) — payload-фильтры; `ruvector-bounded-rag` — token-budget.

---

## 5. Опасности

### 5.1 unsafe (блокирует сборку под `unsafe_code = "forbid"`)
- **`ruvector-rabitq`** — 19 блоков, все `get_unchecked` в `scan.rs:71-99`. Требует переписывания hot-loop.
- **`ruvector-decompiler`** — 3× `std::str::from_utf8_unchecked` (`parser.rs:246,281,292`). Легко исправимо.
- **`ruvector-solver`** — 24, включая unsafe CSR-индексацию (`types.rs:113-118`).
- **`ruvector-core`** — 95 (SIMD NEON/AVX2 в `quantization.rs`), но большинство под `cfg(target_arch)`; модули `advanced_features/*`, `arena`, `lockfree`, `pdx` — safe.
- **`ruvector-graph`** — 14, все в `optimization/{adaptive_radix,memory_pool,simd_traversal}.rs`. Брать граф-ядро (`bm25/schema/index/executor/cypher`) без `optimization/`.
- **`ruvector-mincut`** — 22, но ядро под `cfg_attr(not(feature="wasm"), deny(unsafe_code))` (`lib.rs:119`); unsafe только в `wasm/*` и `optimization/simd_distance.rs` (весь файл под `#[cfg(target_arch="wasm32")]`). **Включать без фичи `wasm`** → safe.
- **`ruvector-temporal-tensor`** — 23 (упаковка/FFI). `ruvector-coherence` — 4 (NEON `spectral.rs`). `ruvector-diskann` — 12. `ruvector-turboquant` — 15. `ruvector-gnn` — 12.
- **Явно безопасные** (`#![forbid(unsafe_code)]`): `hybrid`, `maxsim`, `rairs`, `context`, `graph-condense`, `gnn-rerank`, `perception`, `rvf-forge-core`, `rvforge-registry`. `prime-radiant` — `#![deny(unsafe_code)]` (`lib.rs:147`). `ruvector-mincut` — `deny` вне wasm.

### 5.2 Тяжёлые / нежелательные зависимости
В `prime-agent/Cargo.lock` (348 пакетов) **уже есть**: `sha2`, `thiserror`, `serde`, `serde_json`, `parking_lot`, `smallvec`, `rand`, `rand_xorshift`, `tempfile`, `uuid`, `bumpalo`, `memchr`, `regex`, `once_cell`, `fnv`, `lazy_static`. **Отсутствуют**: `rayon`, `dashmap`, `sha3`, `bincode`, `lz4_flex`, `zstd`, `simsimd`, `redb`, `rusqlite`, `tree-sitter`, `prometheus`, `priority_queue`, `fs4`, `chrono`, `fxhash`, `ahash`, `petgraph`, `roaring`.
- `ruvector-decompiler` тянет `rayon` (в default-фиче `parallel`) и **опционально `ort` = ONNX Runtime** (только фича `neural`) — `ort` тяжёлый, брать без `neural`.
- `ruvector-delta-core` тянет `bincode`+`lz4_flex`+`zstd`+`simsimd` (`Cargo.toml`) — для pa-graph лучше взять только encoding/stream-логику без compression-кодеcов.
- `ruvector-delta-graph`/`sparsifier` тянут `rayon`+`dashmap` (можно отключить фичу `parallel`).
- `ruvector-graph` тянет `redb` (persistent storage) и `roaring`; для SQLite-плана pa-graph это лишнее — брать только алгоритмические модули.
- `ruvector-metrics` тянет `prometheus` — если не нужен Prometheus-экспорт, не брать.
- `ruvector-collections`/`ruvector-context` — `fs4` (file locking), `uuid`, `chrono`.
- `rvf`/`rvf-*` **исключены из root workspace** (`Cargo.toml` exclude) и содержат `rvf-ebpf` (bpf) — брать только `rvf-types`/`rvf-wire`-идеи, не крейты.

### 5.3 Незрелое / недоказанное / «claim-heavy»
- **ADR-статус**: множество крейтов имеют `Status: Proposed` — `namespace-merge` (ADR-299), `query-cache` (301), `streaming-qng` (302), `retrieval-receipt` (304), `bounded-rag`/`adaptive-ann`/`speculative-ann`/`diverse-beam` (272), `spann` (268), `graph-condense` (196/197), `lattice-*`/PIR-серия (305–339). Это дизайн-документы, часто «первая реализация паттерна из paper, без upstream-кода».
- **Отрицательные результаты**: `ruvector-entropy-ann` (ADR-303, closed — negative), `ruvector-domain-expansion`/`sona` (много claims).
- **Placeholder-предупреждение**: `ruvector-core::agenticdb` явно предупреждает, что использует **hash-based placeholder embeddings, а не семантические** (`agenticdb.rs:1`) — не путать с реальными эмбеддингами.
- **`ruvector-cluster`/`raft`/`replication`/`delta-consensus`** — распределённые, для локального форка избыточны.
- **`rvAgent`**: корневой workspace ruvector не содержит `crates/rvAgent` как member (нет `rvAgent/Cargo.toml`; 11 подкрейтов с per-crate `license = "MIT OR Apache-2.0"`); `spawn_parallel` описан как последовательный stub в прошлом отчёте. Брать паттерны, не код.
- **`ruvector-consciousness`/`nervous-system`/`mincut-gated-transformer`/`fpga-transformer`/`sonic-ct`/`photonlayer-*`** — нейро/физика/оптика, к code-intelligence не относятся.
- **Claim-vs-code**: `ruvector-hnsw-repair` таблица в `strategy.rs:7` даёт «recall degradation pp» без указания источника — проверять бенчмарком; `ruvector-graph-condense` честно пишет, что не заявляет accuracy-retention GNN.
- **`ruvector-temporal-tensor`** имеет дублирующиеся определения типов между `tiering.rs` и `store.rs` («Types (to be migrated to crate::store)», `tiering.rs:28`) — признак незавершённого рефакторинга.
- **`micro-hnsw-wasm`** (55 unsafe) и большинство `*-wasm` — не для нативного форка.

### 5.4 Лицензии
- Workspace `license = "MIT"`; крейты без явной лицензии наследуют workspace (`license.workspace = true`) → **MIT**.
- Явно `MIT OR Apache-2.0`: `hybrid`, `maxsim`, `rairs`, `context`(ws MIT), `delta-core/-graph/-index/-consensus`, `proof-gate`, `retrieval-receipt`, `staged-workspace`, `verified`, `temporal-coherence`, `agent-memory`, `spann`, `entropy-ann`, `perception`(ws), `mcp-gate`, `prime-radiant`, `sona`, `rvf-forge-core`.
- **`rvAgent/*`** — per-crate `license = "MIT OR Apache-2.0"` (подтверждено в 11 `Cargo.toml`), но крейты вне root workspace.
- Все — MIT-совместимо с MIT-форком prime-agent. Единственное «нельзя» — код из внешних paper-репозиториев без LICENSE (напр. ReCache, отмечено в ADR-329) — но ruvector переписывал их сам, копируется ruvector-реализация.

---

## Приложение: метод и охват
- Прочитаны `src/lib.rs` + док-комментарии **всех 177** крейтов (`crates/`), плюс пофайловое чтение ~60 целевых модулей.
- Проверены `docs/adr/` (322 файла) — извлечены статусы и решения для ADR-124, 133, 196, 197, 252, 258, 264, 268, 272, 274–278, 297, 299–304, 307, 310–312, 318, 320, 329–331, 334, 337.
- Проверены `docs/research/claude-code-rvsource/` (анализ Claude Code: tree-sitter WASM, permission-система, agent-loop) — ruvector-специфичных крейтов для LSP там нет; tree-sitter присутствует только как WASM-модуль в npm-пакете (`npm/packages/ruvector/src/core/ast-parser.ts`), не как Rust-крейт.
- Верификация `unsafe`: regex по `unsafe {`/`unsafe fn|impl|trait|extern` + точечное чтение контекста (cfg-атрибуты, `forbid/deny`-декларации в lib.rs и Cargo.toml).
- Сборки, тесты, `cargo`, MCP — **не запускались** (по требованию read-only).
