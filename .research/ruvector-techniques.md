# ruvector (cognitum/ruvector): приёмы памяти для агентов

Репо: `/root/vendor-source/ruvector` (MIT, Rust, crates.io `ruvector-core`).
Позиционирование: **Rust-native cognition substrate / persistent adaptive memory
для AI-агентов**. 178 крейтов, 322 ADR, 352 584 узла / 1 200 405 рёбер в графе
(по codebase-memory-mcp). Локальные эмбеддинги, без обязательного API.

## Память: 8 классов (README)
| Класс | Представление | Surface |
|---|---|---|
| Working/session | задача, scratchpad, кэш инструментов, turns, TTL | `WorkingMemory`, `SessionStateIndex` |
| Episodic/Reflexion | trajectory, action, observation, critique, outcome | `EpisodicMemory`, `ReflexionEpisode` |
| Semantic | факты, confidence, source, tags, relations | `VectorDB`, `SemanticFact` |
| Procedural | skills, actions, triggers, policies, Q-values | `ProceduralSkill`, `PolicyMemoryStore` |
| Causal/relational | nodes, edges, hyperedges, Cypher paths | `ruvector-graph` |
| Learning | trajectories, rewards, adapters, EWC | `SONA` |
| Shared | contributions, provenance, voting | `mcp-brain` |
| Auditable | hash-linked entries, snapshots, witnesses | `WitnessLog`, `rvf` |

## Ключевые крейты (LOC)
| Крейт | LOC | Что |
|---|---|---|
| ruvector-core | 32 607 | VectorDB, HNSW, AgenticDB, embeddings |
| ruvector-graph | 25 227 | граф/гиперграф, Cypher |
| ruvector-temporal-tensor | 15 478 | временные тензорные кодеки |
| ruvector-agent-memory | 6 121 | compaction, ledger, fusion, arbitration |
| ruvector-diskann | 4 044 | DiskANN/Vamana |
| ruvector-rabitq | 3 837 | **RaBitQ 1-bit (чистый Rust!)** |
| ruvector-maxsim | 1 964 | multi-vector late interaction |
| ruvector-gnn-rerank | 1 409 | GNN reranking |
| ruvector-pq-search | 1 180 | product quantization |
| ruvector-hybrid | 1 168 | BM25 + dense + RRF |
| ruvector-matryoshka | 1 147 | truncatable embeddings |
| ruvector-lsm-ann | 1 203 | LSM: mutable L0 + compaction L1/L2 |
| ruvector-temporal-coherence | 1 101 | temporal decay + coherence gate |
| ruvector-acorn | 1 080 | predicate-aware ANN |
| ruvector-snapshot | 844 | снапшоты с checksums |

---

## ПРИЁМ №1: RaBitQ — чистый Rust, переносимый как есть ⭐⭐⭐
`crates/ruvector-rabitq` (3 837 LOC, **no BLAS, детерминированный**).
ВНИМАНИЕ: doc-комментарий заявляет «No unsafe», но код использует `unsafe`
`get_unchecked` в hot-loop (scan.rs — 20 мест, index.rs — 1) для устранения
bounds-check. Крейт MIT/Apache-2.0; при переносе — либо сохранить unsafe с
аудитом, либо заменить на safe-индексацию (замедление в горячем пути).
Совпадает с приёмом codebase-memory, но **на Rust** — можно брать напрямую.

### Два оценщика
1. **Symmetric (Charikar)**: query и DB оба 1-bit.
   `est_cos = cos(π·(1 − B/D))`, B = padding-safe XNOR-popcount. O(D/64).
2. **Asymmetric (RaBitQ-2024)**: query f32, DB 1-bit.
   `est_ip = ‖q‖·‖x‖·(1/√D)·Σ sign(x_rot,i)·q_rot,i`. Unbiased на Haar-ротациях.

### Индексы
| Вариант | Хранение | Оценщик | Rerank |
|---|---|---|---|
| `FlatF32Index` | f32 | exact L2 | — |
| `RabitqIndex` | rotation+codes | symmetric | нет |
| `RabitqPlusIndex` | rotation+codes+originals | symmetric | да |
| `RabitqAsymIndex` | rotation+codes[+orig] | asymmetric | опц |

### Ротация (`rotation.rs`)
- `HaarDense`: D×D Haar-uniform (Gram-Schmidt), O(D²), 4·D² байт.
- `HadamardSigned`: **D₁·H·D₂·H·D₃** (3 ±1 диагонали + FWHT), O(D log D),
  хранит только 3·D знаков. Pad до степени 2, затем truncate.
- TurboQuant (arXiv:2504.19874 §3.2): близко к Haar-uniform.

### Padding-safe popcount (`quantize.rs`)
Биты пакуются MSB-first в u64. При `D % 64 != 0` последнее слово имеет
padding-биты; `masked_xnor_popcount` маскирует их (иначе оценщик смещён).
`xnor_popcount` — быстрый путь для aligned D.

### Контракт детерминизма (`kernel.rs`)
- Scan-phase (top-k по Hamming) **бит-воспроизводим** на всех кернелах.
- Rerank-phase (exact L2²) может отличаться в последнем ulp (GPU reduction order).
- `VectorKernel` trait: `id()`, `caps()` (accelerator, min_batch, max_dim,
  deterministic), `scan()`. GPU/SIMD/WASM — отдельные крейты.
- Witness-цепочка anchored на `(data_ref, dim, rotation_seed, rerank_factor, generation)`.

**Перенос**: крейт MIT/Apache-2.0; при копировании решить судьбу unsafe
(сохранить с аудитом или заменить на safe — стоимость в hot-loop).

## ПРИЁМ №2: Гибридный поиск BM25 + dense + RRF/RSF ⭐⭐⭐
`crates/ruvector-hybrid` (1 168 LOC, `#![forbid(unsafe_code)]`).

### BM25 (Robertson, `bm25.rs`)
```
BM25(q,d) = Σ IDF(t)·TF_norm(t,d)
IDF(t)    = ln((N − df(t) + 0.5)/(df(t) + 0.5) + 1)   // +1 против отриц. IDF
TF_norm   = tf·(k1+1)/(tf + k1·(1 − b + b·|d|/avgdl))
k1=1.2, b=0.75
```
Инвертированный список на терм, O(Σ|d|) построение.

### Три fusion-стратегии (`fusion.rs`)
- **RRF** (Reciprocal Rank Fusion): `Σ 1/(k + rank)` — устойчив к разным
  масштабам BM25/cosine.
- **RSF** (Relative Score Fusion).
- **ScoreFusion**: взвешенная линейная (`0.7·cos + 0.3·bm25`).

### Почему RRF (ADR-256)
Linear fusion ломается: BM25 пики (редкий точный терм ×10 медианы), cosine
гладкий (<0.05 в кластере). RRF работает с рангами, не со score.

**Перенос**: крейт целиком — идеальная замена/дополнение к FTS5.

## ПРИЁМ №3: Coherence-Weighted Compaction ⭐⭐⭐
`crates/ruvector-agent-memory` (ADR-252). Проблема: агенты накапливают память,
brute-force latency O(n·d), stale crowding, edge OOM.

### Три политики
| Политика | Сигнал | Новизна |
|---|---|---|
| `LruPolicy` | recency (`last_accessed_at`) | классика |
| `LfuPolicy` | frequency (`access_count`) | классика |
| `CoherencePolicy` | `I = α·recency + β·frequency + γ·coherence` | **research** |

`coherence(m) = max cosine(m, context_window)` — близость к недавним
запросам агента. LLM-free, vector-native, continuous.
Источник идеи: Generative Agents (arXiv:2304.03442), MemoryBank (2305.10250).

### Дополнительные слои
- **ledger/ops**: TARL transactional memory (arXiv:2608.03699) — 5 операций над
  accepted/pending/rejected + witness-записи + proof-gated acceptance.
- **observation/fusion**: cross-source causal fusion (ADR-320) → `CausalEpisodicGraph`.
- **arbitration**: correlation-aware (CAMA, arXiv:2608.19701) — N memories одного
  origin = 1 observation (downgrade-only confidence).

**Перенос**: `CoherencePolicy` — для compaction нашего графа/кэша.

## ПРИЁМ №4: Temporal Decay + Coherence Gate ⭐⭐
`crates/ruvector-temporal-coherence` (1 101 LOC).
Три варианта retrieval:
- `FlatSearch`: чистый cosine.
- `TemporalSearch`: `cosine × exp(-λ·Δt)`.
- `CoherenceSearch`: `cosine × decay × graph_gate`.
`CoherenceGraph`: памяти взаимно-похожие (выше порога) образуют рёбра;
gate = нормализованный in-degree («граф проголосовал за релевантность»).

## ПРИЁМ №5: LSM-ANN (mutable L0 + background compaction) ⭐⭐
`crates/ruvector-lsm-ann` (1 203 LOC) — высокий write-throughput:
mutable L0 (in-memory) + фоновые L1/L2 compaction. Как LSM-дерево, но для ANN.

## ПРИЁМ №6: Matryoshka (truncatable embeddings) ⭐⭐
`crates/ruvector-matryoshka` — coarse-to-fine: поиск по усечённому вектору,
затем уточнение. Экономия на раннем этапе.

## ПРИЁМ №7: MaxSim (multi-vector late interaction) ⭐⭐
`crates/ruvector-maxsim` (1 964 LOC) — ColBERT-style: token/passage-векторы,
late interaction вместо одного pooled-вектора.

## ПРИЁМ №8: Predicate-aware ANN (ACORN) ⭐
`crates/ruvector-acorn` (1 080 LOC) — селективные фильтры без post-filter
recall collapse. Важно для «найти символ с фильтром по типу/файлу».

## ПРИЁМ №9: Product Quantization + temporal tensor ⭐
`ruvector-pq-search` (k-means codebook, 8-16×), `ruvector-temporal-tensor`
(15 478 LOC — низкобитные временные кодеки + reuse сегментов).

## ПРИЁМ №10: Snapshot + RVF witness ⭐
`ruvector-snapshot` — сериализация с compression + checksums.
RVF (canonical format) — copy-on-write branches, hash-linked witness log,
Ed25519-подписи (ADR-029, ADR-134).

## ПРИЁМ №11: Capability-gated retrieval ⭐
`ruvector-capgated` (ADR-268) — 64-битные read-masks внутри поиска:
каждый вектор несёт маску прав, поиск фильтрует по capability.

---

## Что переносить в allspark/pa-graph

| Приём | Крейт | Вердикт |
|---|---|---|
| **RaBitQ (Rust)** | ruvector-rabitq | ⭐⭐⭐ **КОПИРОВАТЬ** (MIT/Apache-2.0; есть unsafe get_unchecked) |
| **BM25+RRF гибрид** | ruvector-hybrid | ⭐⭐⭐ **КОПИРОВАТЬ** (дополняет FTS5) |
| **Coherence compaction** | ruvector-agent-memory | ⭐⭐⭐ переносить политику |
| Temporal decay + gate | ruvector-temporal-coherence | ⭐⭐ переносить |
| LSM-ANN | ruvector-lsm-ann | ⭐ по нужде |
| Matryoshka | ruvector-matryoshka | ⭐ по нужде |
| MaxSim | ruvector-maxsim | опционально |
| ACORN predicate ANN | ruvector-acorn | ⭐ (фильтры символов) |
| PQ + temporal tensor | ruvector-pq-search / temporal-tensor | по нужде |
| Snapshot + witness | ruvector-snapshot | ⭐ (персистентность) |
| Capability-gated | ruvector-capgated | опционально |

### Синтез: три векторных источника → одна система
```
codebase-memory (C):  LSP-surface, closure-repair, 11-сигнальные эмбеддинги, RaBitQ 4-bit
ruvector (Rust):      RaBitQ 1-bit (готовый крейт!), BM25+RRF, coherence compaction, temporal decay
pogocache (C):        layout памяти, 2-random eviction, шардинг, 6-байтные указатели
oh-my-pi/Serena:      LSP-клиент, name_path
```

**Ключевой вывод**: `ruvector` даёт **готовые Rust-крейты** для того, что
codebase-memory реализует на C. Для нашего Rust-форка это **прямой перенос**
вместо портирования C:
- `ruvector-rabitq` → наш векторный поиск (1-bit, детерминированный).
- `ruvector-hybrid` → BM25+RRF (лучше одиночного FTS5).
- `ruvector-agent-memory::CoherencePolicy` → compaction кэша.
- `ruvector-temporal-coherence` → recency-aware recall.

### Лицензия
- workspace `license = "MIT"`.
- ruvector-rabitq: наследует workspace (MIT); doc говорит «no unsafe», но
  фактически есть `unsafe { get_unchecked }` в hot-loop.
- ruvector-hybrid: `MIT OR Apache-2.0`, `#![forbid(unsafe_code)]` ✅.
- ruvector-agent-memory: `MIT OR Apache-2.0`, unsafe=0 ✅.
- ruvector-temporal-coherence: `MIT OR Apache-2.0`.


## ⚠️ КРИТИЧНОЕ ОГРАНИЧЕНИЕ: prime-agent запрещает unsafe

`/root/prime-agent/Cargo.toml:49`: `unsafe_code = "forbid"` (workspace lints).

**Следствие для переноса**:
- `ruvector-rabitq` (22 места `unsafe { get_unchecked }`) — **НЕ соберётся**
  как есть. Нужно:
  (а) переписать hot-loop на safe-индексацию (`packed[b]` вместо
      `*packed.get_unchecked(b)`) — потеря части производительности;
  (б) ИЛИ локальный `#![allow(unsafe_code)]` в крейте — **нарушает политику
      форка** (все крейты pedantic-clean, unsafe forbidden);
  (в) ИЛИ использовать `unsafe` только в изолированном крейте с аудитом
      (потребует обоснования в FORK-ONLY маркерах).
- `ruvector-hybrid` (`#![forbid(unsafe_code)]`) — ✅ соберётся как есть.
- `ruvector-agent-memory` (unsafe=0) — ✅ соберётся как есть.
- `ruvector-temporal-coherence` — проверить (вероятно ✅).

**Рекомендация**: для pa-graph брать **safe-версии** (hybrid, agent-memory,
temporal-coherence) напрямую; rabitq — либо safe-переписать, либо отложить
(у codebase-memory RaBitQ тоже есть, но там C — та же проблема).

Это меняет оценку: **ruvector-rabitq не «копировать целиком»**, а «адаптировать
под forbid(unsafe_code)».
