# АРХИТЕКТУРНАЯ МОДЕЛЬ: синтез Serena + oh-my-pi + codebase-memory в prime-agent

## 0. Метод
Исследовано через MCP `gitnexus` (граф allspark) и `codebase-memory-mcp`
(граф prime-agent, 38 800 узлов / 264 482 ребра). Код НЕ писался (кроме
уже начатого pa-lsp). Цель: модель + оценка встраивания.

## 1. Что есть в prime-agent СЕГОДНЯ (проверено)

### Хранение: чисто файловое
- Нет SQLite, нет графа, нет tree-sitter, нет LSP.
- Сессии: `<agent-dir>/sessions/<uuid>.jsonl`.
- Кэши: JSON-файлы (`plugins.json`, `cron-jobs.json`).
- Единственный «граф» — `context_tree_cache.rs` (599 LOC): кэш дерева
  контекста сессии (children агентов), не граф кода.

### Крейты и слои (fan-in/fan-out из codebase-memory)
```
pa-types      (leaf, 0 deps)          core  fan-in=451
pa-telemetry  (leaf, 0 deps)          core  fan-in=1792
pa-ai         -> pa-types
pa-agent      (leaf, 0 deps)          ~349 nodes
pa-models     -> pa-ai, pa-types      core  fan-in=174
pa-core       -> pa-agent,pa-ai,pa-models,pa-telemetry,pa-types
                                      core  fan-in=3833, fan-out=0  ← ЯДРО
pa-daemon     -> pa-agent,pa-ai,pa-core,pa-models,pa-telemetry,pa-types
                                      fan-in=1294, fan-out=5283
pa-tui        -> pa-types             core  fan-in=1670
pa-cli        -> pa-agent,pa-ai,pa-core,pa-daemon,pa-telemetry,pa-tui,pa-types
```
**pa-core — нижний слой (fan-out=0 в графе, но реально зависит от pa-agent/pa-ai/...).**
Новый крейт кода-интеллекта должен зависеть от pa-types/pa-agent, но НЕ тянуть
pa-core (иначе цикл: pa-core → новый крейт → pa-core).

### Точка внедрения инструментов (подтверждено)
`crates/pa-core/src/session_engine/engine.rs:446`:
```rust
let mut tools = config.tools.clone();
if !tools.iter().any(|tool| tool.name() == PYTHON_TOOL_NAME) {
    ... push ipython ToolDefinitionBridge ...
}
```
`crates/pa-daemon/src/agent_engine/lifecycle.rs:1022`: `tools: vec![]`.
→ Любой нативный инструмент (LSP, graph_search) встаёт сюда, как ipython.

## 2. Три источника идей

| Источник | Ядро | Переносимое |
|---|---|---|
| **Serena** | symbol tools + LSP + name_path | name_path-адресация, diagnostics-to-symbol, 81 LS |
| **oh-my-pi** | TS LSP-клиент | action-tool, ledger, writethrough, servers.json |
| **codebase-memory** | Rust SQLite-граф | lsp_surface, FTS5 BM25, 768-dim int8 vectors, CALLS-confidence, detect_changes, ADR, coverage |

## 3. Доступные зависимости (ВСЁ в cargo-кэше!)

| Crate | Версия | Для чего |
|---|---|---|
| rusqlite | 0.40.2 | SQLite (граф) |
| libsqlite3-sys | 0.38.2 | FFI |
| sqlite-vec | 0.1.7-alpha | векторный поиск в SQLite |
| tantivy | 0.26.2 | полнотекстовый поиск (альтернатива FTS5) |
| petgraph | 0.8.3 | графовые алгоритмы (SCC/cycles/layers) |
| notify | 8.2.0 | FS-watcher (инкрементальная переиндексация) |
| tree-sitter | 0.26.10 | парсинг (индексация) |
| dashmap | 6.2.1 | конкурентные клиенты LSP |
| rayon | 1.12.0 | параллельная индексация |
| async-lsp + lsp-types | 0.2.3 / 0.95.1 | LSP-клиент |
| ignore/globset/walkdir | есть | обход файлов |

## 4. Модель: три уровня (layered)

```
┌─────────────────────────────────────────────────────────────┐
│ УРОВЕНЬ 3: Нативные tools (impl AgentTool)                   │
│  • lsp          — action-tool (10 actions)                   │
│  • graph_search — BM25 + semantic по символам                │
│  • graph_trace  — callers/callees/impact                     │
│  • graph_changes— git-diff → затронутые символы              │
│  → встают в config.tools (engine.rs:446)                     │
└─────────────────────────────────────────────────────────────┘
              ▲ зависимость
┌─────────────────────────────────────────────────────────────┐
│ УРОВЕНЬ 2: pa-graph (НОВЫЙ крейт)                            │
│  • SQLite-граф (nodes/edges/lsp_surface/vectors)             │
│  • BM25 (FTS5 или tantivy) + semantic (sqlite-vec)           │
│  • индексация: pa-ast (tree-sitter) → DEFINES/CALLS          │
│  • detect_changes, coverage, ADR                             │
│  • petgraph: cycles/layers/clusters                          │
└─────────────────────────────────────────────────────────────┘
              ▲ зависимость
┌─────────────────────────────────────────────────────────────┐
│ УРОВЕНЬ 1: pa-lsp (НАЧАТ) + pa-ast (копия pi-ast)            │
│  • pa-lsp: async-lsp клиент, name_path, ledger               │
│  • pa-ast: tree-sitter парсер (из pi-ast, 5.3K LOC)          │
└─────────────────────────────────────────────────────────────┘
              ▲
   pa-types / pa-agent (без pa-core — избегаем цикла)
```

### Почему такой порядок
- `pa-lsp` уже начат (client/servers/symbols/ledger/types). Даёт семантику
  символов через реальный LSP.
- `pa-ast` (из pi-ast) даёт быструю tree-sitter-индексацию без LSP-серверов —
  база для графа (DEFINES/CALLS).
- `pa-graph` объединяет: LSP-поверхность (кэш, как `lsp_surface`) + AST-граф +
  поиск.
- Нативные tools — тонкий фасад.

## 5. Как синтез ложится на prime (конкретные точки)

| Элемент | Куда встаёт | Вторжение |
|---|---|---|
| `lsp` tool | `config.tools` в create_session | 1 место (как ipython) |
| `graph_*` tools | там же | 1 место |
| индексация | при старте сессии/по запросу | фоновая задача |
| SQLite-файл | `<agent-dir>/graph/<project-hash>.db` | новый артефакт |
| lsp_surface-кэш | таблица в том же .db | — |
| prompt | `core.md` (секция про tools) | 1 файл |
| settings | `lsp:{}`, `graph:{}` | как prewarm_ipython_kernel |
| daemon | прокинуть manager (как provisioner) | lifecycle.rs |

### Изоляция от upstream (ключевое требование)
- Новые крейты `pa-lsp`, `pa-graph`, `pa-ast` — отдельные (FORK-ONLY маркеры).
- В существующие файлы — минимальные точечные вставки (1 место в engine.rs,
  1 в lifecycle.rs, core.md, settings).
- Фича-флаги `lsp.enabled`/`graph.enabled` (default off) → upstream-сессии
  ведут себя ровно как раньше.
- Merge-конфликты: только в 3-4 точках, все помечены.

## 6. Оценка (рентабельность)

| Критерий | Оценка |
|---|---|
| Новый код | pa-lsp ~2K + pa-ast ~5.3K + pa-graph ~3-5K + tools ~1K ≈ 11-13K LOC |
| Новые зависимости | rusqlite, sqlite-vec, petgraph, notify, tantivy(опц.), async-lsp, tree-sitter (ВСЕ в кэше) |
| Вес бинаря | +SQLite (~1MB), +tree-sitter грамматики (~5-10MB), +код |
| Вторжение в ядро | 3-4 точки (engine.rs, lifecycle.rs, core.md, settings) |
| Риск upstream merge | низкий (новые крейты + точечные вставки с маркерами) |
| Риск для сессии | низкий (фича-флаги, фоновая индексация, graceful degrade) |
| Готовность LSP | pa-lsp начат (client/servers/symbols/ledger/types) |
| Готовность AST | pi-ast скопирован (untracked) |
| Готовность графа | модель ясна (схема codebase-memory изучена) |

## 7. Фазовый план (модель, не код)

### Фаза 0 (сейчас): модель ✅
Этот документ.

### Фаза 1: pa-lsp довести до компиляции
5 файлов уже есть. Добавить manager.rs + tool.rs + lib.rs. Интеграция в
create_session за фича-флагом. Проверка: rust-analyzer diagnostics в сессии.

### Фаза 2: pa-ast + базовая индексация
Переименовать pa-edit-src-pi-ast → pa-ast. Индексатор: обход файлов →
tree-sitter → узлы (Function/Struct/...) + рёбра (DEFINES/CALLS).
Хранение: SQLite (rusqlite) по схеме codebase-memory.

### Фаза 3: pa-graph (поиск + граф)
FTS5 BM25 + sqlite-vec (768-dim int8) + petgraph (cycles/layers).
`graph_search`, `graph_trace` tools.

### Фаза 4: lsp_surface-кэш + синтез
Объединить LSP-поверхность (pa-lsp) и AST-граф (pa-graph) в один .db.
`detect_changes`, coverage, ADR.

## 8. Ключевые архитектурные решения (требуют подтверждения)

1. **SQLite vs tantivy**: codebase-memory использует SQLite+FTS5+vectors.
   У нас rusqlite+sqlite-vec+tantivy в кэше. Выбор: SQLite (совместимость с
   их схемой, один файл) vs tantivy (мощнее полнотекст, но отдельный индекс).
   → Рекомендация: **SQLite + FTS5 + sqlite-vec** (как codebase-memory).

2. **Индексация: LSP vs tree-sitter**: LSP точен (semantic), но требует
   запущенных серверов. tree-sitter быстр, но приблизителен. codebase-memory
   использует **оба** (AST для структуры, LSP-surface для точных ссылок).
   → Рекомендация: **оба, с приоритетом LSP при доступности**.

3. **Когда индексировать**: eager (при старте сессии) vs lazy (по запросу).
   → Рекомендация: **lazy + фоновая догрузка** (как prewarm ipython).

4. **Где хранить .db**: per-project в `<agent-dir>/graph/<hash>.db` или
   в проекте (`.allspark/graph.db`)?
   → Рекомендация: **agent-dir** (не мусорить в проекте; как gitnexus/.gitnexus).

5. **Изоляция**: фича-флаги default-off — да/нет?
   → Рекомендация: **да** (upstream-совместимость).

## 9. Что НЕ переносим (осознанно)
- Cypher-движок (codebase-memory `query_graph`) — слишком дорого.
- JetBrains-бэкенд Serena.
- Memory-система Serena (у prime есть continual harness).
- `ingest_traces` (не реализован даже в оригинале).
- Полный набор 81 LS (стартуем с 6-10 популярных).


## 10. Проверенные факты (через MCP)

### Точка внедрения и blast radius
- `create_session` (engine.rs:440) — **13 inbound-вызовов** (cli/daemon + тесты).
  Все идут через `SessionEngineConfig` → изменение аддитивно, blast radius низкий.
- `config.tools` уже поддерживает произвольные `Arc<dyn AgentTool>` (проверено:
  `selected_tools: Option<Vec<&str>>`, `has_ipython`/`has_file_access` ветвления).

### Доктрина «один tool» — НЕ жёсткое ограничение
`core.md:3` декларирует «one tool: python_repl», НО:
- Это **prompt-документация**, не валидация кода.
- `AgentContext.tools: Vec<Arc<dyn AgentTool>>` — произвольный список.
- `response.rs:128` шлёт все tools в wire-запрос.
- Уже сегодня `bash` — второй tool в `selected_tools` (через `has_file_access`).
→ Добавление `lsp`/`graph_search` технически бесшовно; нужно лишь:
  (а) обновить `core.md` (секция про native tools),
  (б) ветвление в `system_prompt.rs` по имени tool (шаблон `has_ipython`).

### Хранение
- **SQLite в prime-agent ОТСУТСТВУЕТ** — добавление rusqlite это новая зависимость
  (но она в кэше: rusqlite 0.40.2 + libsqlite3-sys 0.38.2).
- Все данные — файловые (JSONL/JSON). Граф-БД станет первым бинарным хранилищем.

### Зависимости (ВСЕ в cargo-кэше — сборка без сети)
rusqlite 0.40.2, libsqlite3-sys 0.38.2, sqlite-vec 0.1.7-alpha, tantivy 0.26.2,
petgraph 0.8.3, notify 8.2.0, tree-sitter 0.26.10, dashmap 6.2.1, rayon 1.12.0,
async-lsp 0.2.3, lsp-types 0.95.1, ignore/globset/walkdir.

### Граф-движки уже работают рядом
- **GitNexus**: граф allspark (42 767 узлов, 122 115 рёбер, 2115 communities,
  610 processes). Даёт: `impact`, `explain` (taint), `trace`, `pdg_query`
  (program dependence graph), `route_map`, `api_impact`, `shape_check`.
- **codebase-memory-mcp**: граф prime-agent (38 800 узлов, 264 482 ребра).
- Оба — внешние MCP. Наша цель — **нативный аналог внутри allspark**.

## 11. Финальная оценка «как встанет в prime»

| Аспект | Вердикт |
|---|---|
| Точка внедрения | 1 место (engine.rs:446), как ipython ✅ |
| Blast radius | 13 вызовов через Config, аддитивно ✅ |
| Конфликт с доктриной | нет (prompt-текст, не код) ✅ |
| Новые крейты | pa-lsp (начат), pa-ast (копия), pa-graph (новый) |
| Зависимости | все в кэше ✅ |
| Изоляция | фича-флаги + FORK-ONLY маркеры + новые крейты ✅ |
| Хранение | новый .db в agent-dir (не мусорит проект) |
| Риск merge | 3-4 точечных вставки с маркерами |
| Объём | ~11-13K LOC + зависимости |

### Минимальный жизнеспособный путь (рекомендация)
1. **pa-lsp → компилируемый** (5 файлов есть, +manager/tool/lib). Фича-флаг.
2. Интеграция в create_session (1 место). Проверка: LSP-диагностика в сессии.
3. **pa-ast** (переименовать pi-ast) + минимальный индексатор → SQLite.
4. **pa-graph**: FTS5 + sqlite-vec + petgraph. `graph_search`/`graph_trace`.
5. lsp_surface-кэш: объединить LSP + AST.

### Открытые вопросы (для решения)
1. SQLite+FTS5+sqlite-vec (как codebase-memory) vs tantivy?
2. LSP-точность vs tree-sitter-скорость: оба (рекомендуется) или один?
3. Eager vs lazy индексация?
4. .db в agent-dir vs проект?
5. Фича-флаги default-off?


---

# ЧАСТЬ II (обновление): интеграция приёмов pogocache + глубокий codebase-memory

## 12. Что нового из разборов

### Из codebase-memory (DeusData, C11, 257K LOC) — `.research/codebase-memory-deep.md`
| Приём | Суть | Ценность |
|---|---|---|
| **LSP-surface + closure-repair** | surface файла хэшируется; правка тела → пересчёт только файла; правка поверхности → ровно зависимые; замыкание глубины 1 (без fixpoint) | ★★★ |
| **11-сигнальные эмбеддинги** | TF-IDF + Random Indexing + MinHash + API/type sig + AST-profile + dataflow + graph diffusion + Halstead | ★★★ |
| **nomic-embed-code int8** | 31.4 MB блоб: 40 856 токенов × 768d int8 (Apache 2.0) | ★★★ |
| **RaBitQ 4-bit** | XXH3-rotation + FWHT, 768→1024, ~6× экономия памяти | ★★ |
| **MinHash+LSH** | K=64 из AST-триграмм, leaf-only, Jaccard 0.95 | ★★ |
| **CALLS confidence/strategy** | import_map→same_module→unique→suffix + LSP-override (floor 0.6) | ★★ |
| **Схема SQLite** | GENERATED-колонки, contentless FTS5, camelCase-split, coverage-виды | ★★ |

### Из pogocache (tidwall, C11) — `.research/pogocache-techniques.md`
| Приём | Суть | Ценность |
|---|---|---|
| **6-байтные указатели** | 48-bit адрес → бакет 10 байт (6+3+1) | ★★ |
| **Robin Hood + шардинг** | 4096 шардов, high-32 хэша → шард | ★★★ |
| **Entry = 1 аллокация** | var-size поля, size-префикс 1/2/4/8 байт, битовые поля | ★★★ |
| **2-random eviction** | 2 кандидата, старший по timestamp; без LRU-структур | ★★★ |
| **Spinlock + cpu_yield** | pause/yield в spin-loop | ★★ |
| **Refcount для итерации** | atomic rc, без удержания лока | ★★ |
| **Блочный LZ4 + CRC32** | 1 MiB блоки, 16-байтный заголовок | ★ |
| **Cross-compile + SHA-кэш** | 5 платформ, Docker, пропуск по sha | ★ |

## 13. Обновлённая архитектура: pa-graph с учётом приёмов

```
┌──────────────────────────────────────────────────────────────────┐
│ УРОВЕНЬ 3: Нативные tools (impl AgentTool)                        │
│  lsp | graph_search | graph_trace | graph_changes | graph_arch    │
│  → config.tools (engine.rs:446)                                   │
└──────────────────────────────────────────────────────────────────┘
              ▲
┌──────────────────────────────────────────────────────────────────┐
│ УРОВЕНЬ 2: pa-graph                                              │
│                                                                   │
│  ┌─ RAM-слой (pogocache-layout) ──────────────────────────────┐  │
│  │ • Шардированный граф (4096 шардов, high-32 хэша)           │  │
│  │ • Robin Hood внутри шарда                                  │  │
│  │ • Узлы/рёбра = одна аллокация + var-size поля              │  │
│  │ • 6-байтные указатели (48-bit)                             │  │
│  │ • 2-random eviction для кэшей                              │  │
│  │ • Refcount для итерации без lock                           │  │
│  └────────────────────────────────────────────────────────────┘  │
│                                                                   │
│  ┌─ Disk-слой (codebase-memory-схема) ────────────────────────┐  │
│  │ • SQLite: nodes/edges/lsp_surface/file_hashes/coverage     │  │
│  │ • FTS5 contentless + camelCase-split (BM25)                │  │
│  │ • sqlite-vec: 768-dim int8 (или RaBitQ 4-bit)              │  │
│  │ • GENERATED-колонки для индексов по JSON                   │  │
│  └────────────────────────────────────────────────────────────┘  │
│                                                                   │
│  ┌─ Алгоритмы ────────────────────────────────────────────────┐  │
│  │ • closure-repair incremental (LSP-surface)                 │  │
│  │ • 11-сигнальные эмбеддинги (урезать до 5)                  │  │
│  │ • MinHash+LSH (near-clone)                                 │  │
│  │ • CALLS confidence/strategy                                │  │
│  │ • petgraph: cycles/layers/clusters                         │  │
│  │ • detect_changes (git-diff → символы)                      │  │
│  └────────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────────┘
              ▲
┌──────────────────────────────────────────────────────────────────┐
│ УРОВЕНЬ 1: pa-lsp (начат) + pa-ast (копия pi-ast)                │
│  pa-lsp: async-lsp клиент, name_path, ledger, servers            │
│  pa-ast: tree-sitter парсер (DEFINES/CALLS/similarity)           │
└──────────────────────────────────────────────────────────────────┘
              ▲ pa-types / pa-agent (без pa-core)
```

## 14. Ключевой синтез: closure-repair + RAM-граф + eviction

Три системы решают разные задачи, но складываются в одну:
```
codebase-memory: КОГДА переиндексировать (closure-repair, surface-хэш)
pogocache:       КАК держать в памяти (layout, eviction, шардинг)
oh-my-pi/Serena: ЧТО дать модели (LSP-семантика, name_path)
```

### Поток данных (инкрементальная переиндексация)
```
1. FS-событие (notify) или запрос tool
2. file_hashes: mtime+size+sha → что изменилось
3. Для каждого изменённого файла:
   a. pa-ast: tree-sitter → defs (Function/Struct/...)
   b. surface_sha = sha256(канонический JSON defs)
   c. Сравнить со stored lsp_surface.surface_sha
   d. Если байт-идентична → пересчёт ТОЛЬКО файла (тело изменилось)
   e. Если изменилась → pull файлы с рёбрами в него (замыкание глубины 1)
4. Обновить SQLite (nodes/edges, cascade delete)
5. Обновить RAM-кэш (2-random eviction при переполнении)
6. Неопределённость (новые файлы/имена/бюджет) → полный ребилд
```

### Применение pogocache-приёмов к pa-graph
| Приём pogocache | Где в pa-graph |
|---|---|
| Шардинг по high-32 хэша | RAM-граф: узлы по шардам, конкурентный доступ |
| Robin Hood | поиск узла по QN в шарде |
| Entry = 1 аллокация | `Node { time, rc, flags, data[] }` — QN+file+lines+props |
| Size-префикс 1/2/4/8 | размер сериализованных properties |
| 6-байтные указатели | рёбра графа (id → указатель), если свой arena |
| 2-random eviction | кэш LSP-поверхностей и векторов при лимите памяти |
| Spinlock + cpu_yield | короткие критические секции RAM-графа (Rust: parking_lot) |
| Refcount | итерация графа (обход соседей) без удержания лока |
| Блочный LZ4 | персистентность .db (бэкап снапшота) |

## 15. Обновлённая оценка

| Критерий | Оценка (v2) |
|---|---|
| Новый код | pa-lsp ~2K + pa-ast ~5.3K + pa-graph ~5-7K (был 3-5K; +RAM-слой, +алгоритмы) + tools ~1.5K ≈ **14-16K LOC** |
| Зависимости | rusqlite, sqlite-vec, petgraph, notify, tree-sitter, async-lsp, dashmap, rayon, lz4 (опц.) — **все в кэше** |
| Вес бинаря | +SQLite ~1MB, +tree-sitter грамматики 5-10MB, +nomic-блоб 31MB (опц.), +код |
| Вторжение в ядро | 3-4 точки (engine.rs, lifecycle.rs, core.md, settings) |
| Риск merge | низкий (новые крейты + маркеры) |
| Риск сессии | низкий (фича-флаги, lazy-индексация, graceful degrade) |
| Готовность | pa-lsp начат; схема/алгоритмы изучены |

### Дополнительные решения (v2)
6. **RAM-слой**: делать свой шардированный граф (pogocache-layout) или
   использовать `dashmap` + `petgraph`? → Рекомендация: **начать с dashmap+petgraph,
   оптимизировать layout позже** (преждевременная оптимизация памяти опасна).
7. **Эмбеддинги**: все 11 сигналов или урезать? → **урезать до 4-5**
   (TF-IDF, Random Indexing, MinHash, AST-profile, module-proximity).
8. **nomic-блоб (31 MB)**: включать? → **опционально** (фича-флаг; +31MB бинаря).
9. **RaBitQ**: сразу или int8? → **int8 сначала** (проще), RaBitQ при нужде.
10. **closure-repair**: требует «чистой функции per-file» — LSP-клиент это
    нарушает. → **базировать на pa-ast (детерминированный), LSP — как override**.

## 16. Обновлённый фазовый план

### Фаза 0 ✅ модель (этот документ, v2)
### Фаза 1: pa-lsp → компилируемый (5 файлов + manager/tool/lib), фича-флаг
### Фаза 2: pa-ast → индексатор (tree-sitter → defs), **surface-хэш**
### Фаза 3: pa-graph disk-слой (SQLite по схеме codebase-memory)
### Фаза 4: pa-graph RAM-слой (dashmap+petgraph, потом оптимизация)
### Фаза 5: closure-repair incremental (главный приём)
### Фаза 6: поиск (FTS5 BM25 + эмбеддинги) + `graph_*` tools
### Фаза 7: LSP-surface интеграция + detect_changes + coverage + ADR
### Фаза 8 (опц.): pogocache-layout оптимизация памяти, nomic-блоб, RaBitQ

## 17. Итоговый синтез (три источника → одна система)

```
┌─────────────────────────────────────────────────────────────┐
│                    pa-graph / pa-lsp                        │
│                                                              │
│  WHEN  ← codebase-memory: closure-repair, surface-хэш       │
│  HOW   ← pogocache: layout, 2-random eviction, шардинг      │
│  WHAT  ← oh-my-pi/Serena: LSP-семантика, name_path          │
│                                                              │
│  Итого: нативный Rust-граф кода + LSP + поиск, встроенный   │
│  в prime-agent как нативные tools за фича-флагами.          │
└─────────────────────────────────────────────────────────────┘
```


---

# ЧАСТЬ III (обновление): интеграция приёмов ruvector

## 18. Что нового из ruvector — `.research/ruvector-techniques.md`

ruvector (MIT, Rust, 178 крейтов, 322 ADR, 352K узлов/1.2M рёбер по
codebase-memory-mcp) — **Rust-native memory substrate для агентов**.

| Крейт | LOC | Приём | unsafe | Лицензия |
|---|---|---|---|---|
| **ruvector-rabitq** | 3 837 | RaBitQ 1-bit (2 оценщика, 4 индекса, Haar/Hadamard ротация) | **22** | MIT |
| **ruvector-hybrid** | 1 168 | BM25 (k1=1.2,b=0.75) + dense + **RRF/RSF** | 0 (`forbid`) | MIT/Apache |
| **ruvector-agent-memory** | 6 121 | **CoherencePolicy** `I=α·rec+β·freq+γ·coh` | 0 | MIT/Apache |
| **ruvector-temporal-coherence** | 1 101 | `cos × exp(-λΔt) × graph_gate` | 0 | MIT/Apache |
| ruvector-lsm-ann | 1 203 | mutable L0 + background L1/L2 | — | — |
| ruvector-matryoshka | 1 147 | truncatable embeddings (coarse→fine) | 0 | — |
| ruvector-maxsim | 1 964 | multi-vector late interaction | 0 (`forbid`) | — |
| ruvector-acorn | 1 080 | predicate-aware ANN (фильтры) | 0 | — |
| ruvector-pq-search | 1 180 | product quantization | 0 | — |
| ruvector-snapshot | 844 | снапшоты + checksums | — | — |
| ruvector-gnn-rerank | 1 409 | GNN reranking | 0 (`forbid`) | — |

### Ключевое открытие: Rust vs C
ruvector даёт **готовые Rust-крейты** для того, что codebase-memory реализует
на C. Для нашего Rust-форка это **прямой перенос** вместо портирования C:
- `ruvector-hybrid` → BM25+RRF (лучше одиночного FTS5; ADR-256).
- `ruvector-agent-memory::CoherencePolicy` → compaction кэша (ADR-252).
- `ruvector-temporal-coherence` → recency-aware recall (ADR-211).
- `ruvector-rabitq` → векторный поиск (но см. ограничение ниже).

## 19. ⚠️ КРИТИЧНОЕ ОГРАНИЧЕНИЕ: forbid(unsafe_code)

`/root/prime-agent/Cargo.toml:49`: **`unsafe_code = "forbid"`** (workspace lints).
Все крейты форка — pedantic-clean, unsafe запрещён.

| Крейт | unsafe | Собирается в prime? |
|---|---|---|
| ruvector-rabitq | **22** (`get_unchecked` в hot-loop) | ❌ НЕТ |
| ruvector-hybrid | 0 (`#![forbid(unsafe_code)]`) | ✅ да |
| ruvector-agent-memory | 0 | ✅ да |
| ruvector-temporal-coherence | 0 | ✅ да |
| ruvector-matryoshka / acorn / pq-search | 0 | ✅ да |

**Варианты для rabitq**:
(а) переписать hot-loop на safe-индексацию (`packed[b]`) — потеря части
    производительности (bounds-check);
(б) локальный `#![allow(unsafe_code)]` — **нарушает политику форка**;
(в) изолированный крейт с аудитом + FORK-ONLY обоснование.

**Рекомендация**: начать с safe-крейтов (hybrid, agent-memory,
temporal-coherence). RaBitQ — либо safe-переписать, либо отложить.

## 20. Обновлённая архитектура pa-graph (v3, с ruvector)

```
┌──────────────────────────────────────────────────────────────────────┐
│ УРОВЕНЬ 3: Нативные tools (impl AgentTool)                            │
│  lsp | graph_search | graph_trace | graph_changes | graph_arch        │
└──────────────────────────────────────────────────────────────────────┘
              ▲
┌──────────────────────────────────────────────────────────────────────┐
│ УРОВЕНЬ 2: pa-graph                                                  │
│                                                                       │
│  ┌─ RAM-слой (pogocache-layout) ──────────────────────────────────┐  │
│  │ шардированный граф, Robin Hood, entry=1 аллокация,             │  │
│  │ 6-байтные указатели, 2-random eviction, refcount               │  │
│  └────────────────────────────────────────────────────────────────┘  │
│                                                                       │
│  ┌─ Disk-слой (codebase-memory-схема) ────────────────────────────┐  │
│  │ SQLite: nodes/edges/lsp_surface/file_hashes/coverage           │  │
│  │ GENERATED-колонки, contentless FTS5 + camelCase-split          │  │
│  └────────────────────────────────────────────────────────────────┘  │
│                                                                       │
│  ┌─ Поиск (ruvector) ─────────────────────────────────────────────┐  │
│  │ • BM25+RRF гибрид (ruvector-hybrid) — лучше одиночного FTS5    │  │
│  │ • RaBitQ 1-bit (ruvector-rabitq) — safe-версия или отложить    │  │
│  │ • ACORN predicate-ANN — фильтры по типу/файлу символа          │  │
│  │ • Matryoshka — coarse→fine (по нужде)                          │  │
│  └────────────────────────────────────────────────────────────────┘  │
│                                                                       │
│  ┌─ Жизненный цикл (ruvector-agent-memory) ───────────────────────┐  │
│  │ • CoherencePolicy: I=α·recency+β·frequency+γ·coherence         │  │
│  │ • Temporal decay: cos × exp(-λΔt) × graph_gate                 │  │
│  │ • 2-random eviction (pogocache) для жёсткого лимита            │  │
│  └────────────────────────────────────────────────────────────────┘  │
│                                                                       │
│  ┌─ Алгоритмы (codebase-memory) ──────────────────────────────────┐  │
│  │ closure-repair incremental, 11→5 сигналов эмбеддингов,         │  │
│  │ MinHash+LSH, CALLS confidence/strategy, detect_changes, ADR    │  │
│  └────────────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────────────┘
              ▲
┌──────────────────────────────────────────────────────────────────────┐
│ УРОВЕНЬ 1: pa-lsp (начат) + pa-ast (копия pi-ast)                    │
└──────────────────────────────────────────────────────────────────────┘
              ▲ pa-types / pa-agent (без pa-core)
```

## 21. Четыре источника → одна система (финальный синтез)

```
┌──────────────────────────────────────────────────────────────────┐
│                       pa-graph / pa-lsp                          │
│                                                                   │
│  WHEN   ← codebase-memory: closure-repair, surface-хэш            │
│  HOW    ← pogocache: layout, 2-random eviction, шардинг           │
│  SEARCH ← ruvector: BM25+RRF, RaBitQ, coherence, temporal decay   │
│  WHAT   ← oh-my-pi/Serena: LSP-семантика, name_path               │
│                                                                   │
│  Итого: нативный Rust-граф кода + LSP + гибридный поиск +         │
│  memory-lifecycle, встроенный в prime-agent как нативные tools    │
│  за фича-флагами. Все зависимости — в cargo-кэше.                │
└──────────────────────────────────────────────────────────────────┘
```

### Разделение ответственности
| Вопрос | Источник | Приём |
|---|---|---|
| Когда переиндексировать | codebase-memory | closure-repair, surface-хэш |
| Как держать в RAM | pogocache | шардинг, 2-random, 6-байт указатели |
| Как искать | ruvector | BM25+RRF, RaBitQ, ACORN |
| Что забывать | ruvector | CoherencePolicy, temporal decay |
| Что дать модели | oh-my-pi/Serena | LSP, name_path |

## 22. Обновлённая оценка (v3)

| Критерий | Оценка (v3) |
|---|---|
| Новый код | pa-lsp ~2K + pa-ast ~5.3K + pa-graph ~6-8K (v3: +поиск ruvector) + tools ~1.5K ≈ **15-17K LOC** |
| Прямые копии (safe) | ruvector-hybrid 1.2K, ruvector-agent-memory 6.1K (частично), ruvector-temporal-coherence 1.1K |
| Зависимости | rusqlite, sqlite-vec, petgraph, notify, tree-sitter, async-lsp, dashmap, rayon — **все в кэше** |
| Вес бинаря | +SQLite ~1MB, +tree-sitter 5-10MB, +код |
| Вторжение в ядро | 3-4 точки (engine.rs, lifecycle.rs, core.md, settings) |
| Риск merge | низкий (новые крейты + маркеры) |
| Риск сессии | низкий (фича-флаги, lazy-индексация) |
| **Новое: unsafe-риск** | rabitq требует safe-переписки или изоляции |

### Дополнительные решения (v3)
11. **BM25+RRF vs FTS5**: ruvector-hybrid даёт готовый Rust BM25+RRF →
    **предпочесть его** одиночному FTS5 (лучше качество fusion).
12. **RaBitQ**: **отложить до Фазы 8** (требует safe-переписки из-за
    forbid(unsafe_code)); начать с int8-векторов + sqlite-vec.
13. **CoherencePolicy**: **включить** (safe, готовый крейт) — compaction кэша
    по recency+frequency+coherence.
14. **Temporal decay**: включить для долгоживущих сессий.

## 23. Обновлённый фазовый план (v3)

### Фаза 0 ✅ модель (этот документ, v3)
### Фаза 1: pa-lsp → компилируемый, фича-флаг
### Фаза 2: pa-ast → индексатор (tree-sitter → defs), surface-хэш
### Фаза 3: pa-graph disk-слой (SQLite по схеме codebase-memory)
### Фаза 4: pa-graph RAM-слой (dashmap+petgraph)
### Фаза 5: closure-repair incremental (главный приём)
### Фаза 6: поиск — **BM25+RRF (ruvector-hybrid)** + int8-векторы
### Фаза 7: LSP-surface интеграция + detect_changes + coverage + ADR
### Фаза 7.5: **CoherencePolicy + temporal decay (ruvector)** — memory lifecycle
### Фаза 8 (опц.): RaBitQ safe-версия, pogocache-layout, nomic-блоб, ACORN

## 24. Итоговый реестр приёмов (все источники)

### codebase-memory (C)
LSP-surface, closure-repair, 11-сигнальные эмбеддинги, RaBitQ 4-bit,
MinHash+LSH, CALLS-confidence, схема SQLite (GENERATED, FTS5, coverage).

### pogocache (C)
6-байтные указатели, Robin Hood+шардинг, entry=1 аллокация, sixpack,
spinlock+batch-ptr, 2-random eviction, блочный LZ4, cross-compile+SHA-кэш.

### ruvector (Rust) — ПРЯМОЙ ПЕРЕНОС
RaBitQ (есть unsafe), BM25+RRF (safe), CoherencePolicy (safe),
temporal decay+gate (safe), LSM-ANN, Matryoshka, MaxSim, ACORN, PQ, snapshot.

### oh-my-pi/Serena
LSP-клиент (async-lsp), name_path, diagnostics-to-symbol, action-tool, ledger.

### Верификация MCP
- GitNexus: граф allspark (42 767 узлов, 122 115 рёбер).
- codebase-memory-mcp: граф prime-agent (38 800 узлов, 264 482 ребра) +
  граф ruvector (352 584 узла, 1 200 405 рёбер).
