# МАТРИЦА ПРОЕКТИРОВАНИЯ: функции/фишки по источникам → реализация в prime

## 0. Как читать
- **Источник**: Serena / oh-my-pi / codebase-memory / pogocache / ruvector.
- **Фишка**: конкретная функция/приём.
- **Сейчас в prime**: как реализовано сегодня (место, язык).
- **Как реализовать**: Rust-крейт / Python-skill / host_request / tool.
- **Точка интеграции**: файл:строка.
- **Пересечения**: с чем конфликтует/делит API.
- **Ось контроля**: что нужно для управления.

---

## 1. СВОДНАЯ МАТРИЦА (фишка → реализация)

### 1.1 LSP / семантика кода (источник: oh-my-pi + Serena)

| # | Фишка | Сейчас в prime | Как реализовать | Точка интеграции | Пересечения | Ось |
|---|---|---|---|---|---|---|
| L1 | LSP-клиент (spawn/init/query) | НЕТ | Rust `pa-lsp` (async-lsp) | новый крейт | — | lsp.enabled |
| L2 | `lsp` action-tool (10 actions) | НЕТ | Rust `impl AgentTool` | config.tools (engine.rs:446) | ipython, prompt | lsp.actions |
| L3 | name_path адресация | НЕТ | Rust (порт Serena) | pa-lsp/symbols.rs | — | — |
| L4 | diagnostics (pull/push) | НЕТ | Rust LSP | pa-lsp | edit-триггер | lsp.diagnostics |
| L5 | rename (WorkspaceEdit) | НЕТ | Rust LSP | pa-lsp | edit, apply-policy | lsp.rename |
| L6 | references/definition/hover | НЕТ | Rust LSP | pa-lsp | graph_search | — |
| L7 | servers.json (57 серверов) | НЕТ | Rust const | pa-lsp/servers.rs | — | lsp.servers |
| L8 | diagnostics-ledger (дедуп) | НЕТ | Rust | pa-lsp/ledger.rs | — | — |
| L9 | diagnostics-to-symbol | НЕТ | Rust (порт Serena) | pa-lsp | graph (name_path) | — |
| L10 | writethrough (did_change) | НЕТ | Rust LSP | pa-lsp + edit | edit-триггер | lsp.writethrough |
| L11 | LSP-surface кэш | НЕТ | Rust + SQLite | pa-graph | closure-repair | lsp.surface |

### 1.2 Граф кода (источник: codebase-memory + GitNexus)

| # | Фишка | Сейчас | Как | Точка | Пересечения | Ось |
|---|---|---|---|---|---|---|
| G1 | SQLite-граф (nodes/edges) | НЕТ | Rust rusqlite | pa-graph | — | graph.enabled |
| G2 | Индексация (tree-sitter) | НЕТ | Rust pa-ast | pa-graph | pa-ast | graph.index |
| G3 | `graph_search` (BM25+RRF) | rg/Python | Rust ruvector-hybrid | config.tools | FTS5 | graph.search |
| G4 | `graph_trace` (callers/callees) | НЕТ | Rust pa-graph | config.tools | L6 | graph.trace |
| G5 | `graph_changes` (git→символы) | git diff | Rust pa-graph | config.tools | git | graph.changes |
| G6 | `graph_arch` (overview/layers) | НЕТ | Rust petgraph | config.tools | — | graph.arch |
| G7 | closure-repair incremental | НЕТ | Rust (порт codebase-memory) | pa-graph | L11 | graph.incremental |
| G8 | CALLS confidence/strategy | НЕТ | Rust | pa-graph | — | — |
| G9 | coverage-честность | НЕТ | Rust | pa-graph | — | graph.coverage |
| G10 | ADR (manage_adr) | НЕТ | Rust/SQLite | pa-graph | harness memory | graph.adr |
| G11 | detect_changes | НЕТ | Rust | pa-graph | G5 | — |

### 1.3 Поиск/векторы (источник: codebase-memory + ruvector)

| # | Фишка | Сейчас | Как | Точка | Пересечения | Ось |
|---|---|---|---|---|---|---|
| V1 | FTS5 BM25 (camelCase-split) | НЕТ | SQLite FTS5 | pa-graph | G3 | graph.fts |
| V2 | BM25+RRF гибрид | НЕТ | Rust ruvector-hybrid | pa-graph | G3 | graph.hybrid |
| V3 | Эмбеддинги (11→5 сигналов) | НЕТ | Rust | pa-graph | — | graph.semantic |
| V4 | int8-векторы (768d) | НЕТ | sqlite-vec | pa-graph | — | graph.vectors |
| V5 | RaBitQ 1-bit | НЕТ | Rust ruvector-rabitq (safe-версия) | pa-graph | — | graph.rabitq |
| V6 | MinHash+LSH (near-clone) | НЕТ | Rust | pa-graph | — | graph.similarity |
| V7 | nomic-блоб (31MB) | НЕТ | Rust + данные | pa-graph | — | graph.nomic |
| V8 | ACORN predicate-ANN | НЕТ | Rust ruvector-acorn | pa-graph | — | graph.acorn |
| V9 | Matryoshka (coarse→fine) | НЕТ | Rust ruvector-matryoshka | pa-graph | — | — |
| V10 | MaxSim (late interaction) | НЕТ | Rust ruvector-maxsim | pa-graph | — | — |

### 1.4 Память/жизненный цикл (источник: ruvector + codebase-memory)

| # | Фишка | Сейчас | Как | Точка | Пересечения | Ось |
|---|---|---|---|---|---|---|
| M1 | CoherencePolicy (compaction) | compaction (токены) | Rust ruvector-agent-memory | pa-graph | compaction | graph.compaction |
| M2 | temporal decay + gate | НЕТ | Rust ruvector-temporal-coherence | pa-graph | — | graph.decay |
| M3 | 2-random eviction | НЕТ | Rust (порт pogocache) | pa-graph | — | graph.eviction |
| M4 | Witness log (audit) | НЕТ | Rust ruvector-snapshot | pa-graph | — | graph.witness |
| M5 | LSM-ANN (write-heavy) | НЕТ | Rust ruvector-lsm-ann | pa-graph | — | — |

### 1.5 Edit (источник: oh-my-pi + Serena)

| # | Фишка | Сейчас | Как | Точка | Пересечения | Ось |
|---|---|---|---|---|---|---|
| E1 | exact-match edit | Python skill (69 строк) | Rust нативный tool (гибрид) | config.tools + skill | prompt | edit.mode |
| E2 | AST-правки (pi-edit modes) | НЕТ | Rust pi-edit | edit tool | pa-ast | edit.ast |
| E3 | stale-tag guard | НЕТ | Rust (порт hashline) | edit tool | read-provenance | edit.stale |
| E4 | seen-lines guard | НЕТ | Rust | edit tool | read-provenance | edit.seen |
| E5 | no-op loop limit (3) | частично | Rust | edit tool | — | edit.noop |
| E6 | diagnostics-after-edit | НЕТ | Rust (LSP) | edit → L10 | L4 | edit.diagnostics |
| E7 | multi-file atomic stage | НЕТ | Rust | edit tool | E1 | edit.batch |

### 1.6 Память процессора/памяти (источник: pogocache)

| # | Фишка | Сейчас | Как | Точка | Пересечения | Ось |
|---|---|---|---|---|---|---|
| P1 | Шардированный RAM-граф | НЕТ | Rust (dashmap→свой) | pa-graph | G1 | graph.shards |
| P2 | Robin Hood hashmap | НЕТ | Rust | pa-graph | P1 | — |
| P3 | 6-байтные указатели | НЕТ | Rust (опц.) | pa-graph | — | graph.layout |
| P4 | Entry=1 аллокация | НЕТ | Rust arena | pa-graph | — | — |
| P5 | Spinlock + cpu_yield | parking_lot | Rust | pa-graph | — | — |
| P6 | Refcount для итерации | НЕТ | Rust Arc | pa-graph | — | — |
| P7 | Блочный LZ4 (персистентность) | JSONL | Rust lz4 | pa-graph | snapshot | graph.snapshot |
| P8 | Cross-compile + SHA-кэш | есть (installer) | build-скрипт | упаковка | installer | build.* |

---

## 2. ОСИ КОНТРОЛЯ (что нужно добавить)

### 2.1 Функциональные оси (feature flags)
| Ось | Тип | Default | Что контролирует |
|---|---|---|---|
| `lsp.enabled` | bool | false | весь LSP-слой |
| `lsp.servers` | map | auto | какие серверы разрешены |
| `lsp.actions` | list | all | какие actions доступны |
| `lsp.writethrough` | bool | true | did_change после edit |
| `lsp.diagnosticsAfterEdit` | bool | true | диагностика после правки |
| `graph.enabled` | bool | false | весь граф-слой |
| `graph.search` | enum | hybrid | bm25/vector/hybrid |
| `graph.semantic` | bool | false | эмбеддинги |
| `graph.incremental` | bool | true | closure-repair |
| `graph.rabitq` | bool | false | 1-bit квантизация |
| `graph.nomic` | bool | false | 31MB блоб |
| `graph.compaction` | enum | coherence | lru/lfu/coherence |
| `graph.decay` | bool | false | temporal decay |
| `edit.mode` | enum | exact | exact/hashline/ast |
| `edit.guards` | bool | true | stale/seen/no-op |

### 2.2 Операционные оси (ресурсы)
| Ось | Тип | Default | Что контролирует |
|---|---|---|---|
| `graph.maxMemoryMb` | int | 512 | лимит RAM-графа |
| `graph.eviction` | enum | 2random | 2random/lru/lfu |
| `graph.indexBudgetPct` | int | 30 | closure-repair бюджет |
| `graph.shards` | int | auto | число шардов |
| `lsp.idleTimeoutMs` | int | 300000 | остановка LSP-клиентов |
| `lsp.callTimeoutMs` | int | 20000 | таймаут запросов |
| `graph.reindexOnStart` | bool | false | eager vs lazy |
| `graph.watch` | bool | false | FS-watcher |

### 2.3 Оси изоляции (upstream-совместимость)
| Ось | Тип | Default | Что контролирует |
|---|---|---|---|
| `graph.dbPath` | path | agent-dir | где .db |
| `graph.scope` | enum | project | project/global |
| `lsp.workspaceRoot` | path | cwd | корень LSP |
| `fork.*` | — | — | FORK-ONLY маркеры |

### 2.4 Оси наблюдаемости (для отладки)
| Ось | Тип | Default | Что контролирует |
|---|---|---|---|
| `graph.trace` | bool | false | логи индексации |
| `lsp.trace` | bool | false | LSP-трафик |
| `graph.stats` | — | on | coverage/status |
| `graph.generation` | — | auto | версия индекса |

---

## 3. ПЕРЕСЕЧЕНИЯ (cross-cutting)

### 3.1 edit ↔ lsp ↔ graph
```
edit (правка файла)
  → host_request "lsp.did_change" → pa-lsp обновляет документ
  → diagnostics-ledger → новые диагностики
  → pa-graph: инвалидация surface (closure-repair)
  → graph_search переиндексирует изменённый файл
```
**Общий контракт**: `FileChangeEvent { path, old_hash, new_hash }`.

### 3.2 lsp ↔ graph (name_path)
```
lsp(action="symbols") → дерево символов
  → pa-graph: name_path как ключ адресации
  → graph_trace по name_path
```

### 3.3 search ↔ graph
```
graph_search (BM25+RRF) → использует nodes_fts + vectors из pa-graph
  → тот же .db, тот же индекс
```

### 3.4 compaction ↔ graph (memory lifecycle)
```
Session compaction (токены) — существующая
Graph compaction (CoherencePolicy) — НОВАЯ, для кэша графа
  → разные уровни, не конфликтуют
```

### 3.5 harness memory ↔ graph ADR
```
Continual harness (prompt/memory/skill/subagent) — существующая
Graph ADR (manage_adr) — архитектурные решения в .db
  → ADR может ссылаться на harness, но не дублирует
```

### 3.6 subagents ↔ graph
```
rlm.spawn → дочерний агент → тот же .db (per-project)
  → родитель может запросить impact у ребёнка
```

### 3.7 prompt ↔ tools
```
core.md: "one tool: python_repl" — ДОКУМЕНТАЦИЯ
  → добавить секцию про native tools (lsp/graph_*)
  → system_prompt.rs: ветвление по имени (шаблон has_ipython)
```

---

## 4. ЧТО ЕЩЁ НУЖНО ДОБАВИТЬ (контроль/наблюдаемость)

### 4.1 Инструменты контроля (для агента)
| Инструмент | Назначение | Источник идеи |
|---|---|---|
| `graph_status` | статус индекса (generation, coverage, размер) | codebase-memory index_status |
| `graph_coverage` | честность покрытия (parse_partial/skipped) | codebase-memory check_index_coverage |
| `lsp_status` | какие LSP-серверы живы | omp status action |
| `lsp_capabilities` | что умеет сервер | omp capabilities |

### 4.2 Метрики (для наблюдаемости)
| Метрика | Что измеряет | Куда |
|---|---|---|
| `graph.index_ms` | время индексации | telemetry |
| `graph.query_ms` | время запроса | telemetry |
| `graph.cache_hit_rate` | попадания кэша | telemetry |
| `lsp.call_ms` | время LSP-запроса | telemetry |
| `lsp.servers_live` | живые серверы | telemetry |
| `edit.guard_rejections` | сработавшие guards | telemetry |

### 4.3 Предохранители (safety)
| Предохранитель | Что защищает |
|---|---|
| Лимит размера .db | диск |
| Лимит RAM-графа | память |
| Таймаут LSP-вызова | зависание |
| Idle-остановка LSP | процессы |
| Budget closure-repair | время индексации |
| Fallback на rg/Python | недоступность графа |
| Graceful degrade | LSP-сервер не найден |

### 4.4 Версионирование (совместимость)
| Аспект | Схема |
|---|---|
| Схема .db | `graph.schemaVersion` |
| Формат surface | `surface.v` (как codebase-memory) |
| Миграция | rebuild при несовпадении |
| LSP-протокол | lsp-types версия |

---

## 5. МАТРИЦА РИСКОВ

| Риск | Вероятность | Влияние | Митигация |
|---|---|---|---|
| forbid(unsafe_code) ломает rabitq | высокая | средний | safe-версия/отложить |
| Конфликт с upstream merge | средний | высокий | FORK-ONLY + фича-флаги |
| Тяжёлая сборка (deps) | средний | средний | все в кэше |
| Рост бинаря (+31MB nomic) | средний | низкий | опционально |
| LSP-сервер не установлен | высокая | низкий | graceful degrade |
| RAM-граф OOM | средний | высокий | 2-random eviction + лимит |
| Зависание LSP | средний | средний | таймауты + idle-stop |
| .db повреждён | низкий | высокий | WAL + snapshot |
| Промпт раздут | средний | средний | компактные описания |
| Субагенты конфликтуют за .db | средний | средний | WAL + per-project |

---

## 6. ПОРЯДОК РЕАЛИЗАЦИИ (по зависимостям)

```
Фаза 1: pa-lsp (L1-L8) → lsp tool (L2)
   ↓ (независимо)
Фаза 2: pa-ast (G2) → индексатор
   ↓
Фаза 3: pa-graph disk (G1) + surface (L11)
   ↓
Фаза 4: pa-graph RAM (P1-P6)
   ↓
Фаза 5: closure-repair (G7)
   ↓
Фаза 6: поиск (V1-V2, G3) + trace (G4)
   ↓
Фаза 7: LSP-surface интеграция (L4, L9, L10) + changes (G5)
   ↓
Фаза 7.5: memory lifecycle (M1-M3)
   ↓
Фаза 8: edit-гибрид (E1-E7)
   ↓
Фаза 9 (опц.): RaBitQ (V5), nomic (V7), ACORN (V8)
```

### Критический путь
`pa-ast` → `pa-graph` → `closure-repair` → `graph_search`/`graph_trace`.
`pa-lsp` — параллельная ветка (не зависит от графа).

---

## 7. ОТКРЫТЫЕ ВОПРОСЫ (для решения)

1. **edit-гибрид**: Python-скилл → Rust-tool. Сохранить Python-фасад или убрать?
2. **RAM-граф**: dashmap+petgraph (просто) vs pogocache-layout (оптимально)?
3. **Эмбеддинги**: 5 сигналов или все 11?
4. **nomic-блоб**: включать (+31MB) или нет?
5. **RaBitQ**: safe-переписка или отложить?
6. **БД**: agent-dir vs проект?
7. **Субагенты**: один .db на проект или на сессию?
8. **Промпт**: сколько токенов готовы отдать под описания tools?
9. **Fallback**: при недоступности графа — молча rg или явная ошибка?
10. **Совместимость**: фича-флаги default-off — точно?


---

## 8. МЕХАНИКИ ИНТЕГРАЦИИ (детально, верифицировано)

### 8.1 Четыре способа добавить функциональность

| # | Механика | Язык | Как работает | Пример в prime |
|---|---|---|---|---|
| A | **Нативный Rust-tool** | Rust | `impl AgentTool` → `config.tools` → wire + prompt | ipython (engine.rs:446) |
| B | **Python-skill** | Python | `skills/<n>/{SKILL.md,pyproject,src/}` → биндится в REPL namespace | edit, websearch, refine |
| C | **host_request** | Rust↔Python | Python: `await host_request("kind", payload)`; Rust: `handlers.register("kind", fn)` | rlm.run, mcp.* |
| D | **MCP server** | внешний | settings.json → mcpServers → Python-клиент | gitnexus, codebase-memory |

### 8.2 Детали механики A (нативный tool)
```
config.tools: Vec<Arc<dyn AgentTool>>   (engine.rs:33)
  → lifecycle.rs:1022 tools: vec![]
  → engine.rs:446 let mut tools = config.tools.clone();
  → push ipython (существующий образец)
  → response.rs:128: tools → wire-запрос провайдеру
  → system_prompt.rs: selected_tools → prompt
AgentTool trait (types.rs:498):
  name(), description(), parameters() (JSON Schema), label(),
  prepare_arguments(), accepts_alias(), execute(), execution_mode()
```
**Когда**: нужен нативный Rust-инструмент, видимый модели как tool.

### 8.3 Детали механики B (Python-skill)
```
skills/<name>/pyproject.toml → detect_python_skill (discovery.rs:35)
  → SkillPythonMetadata { import_name, package_path, pyproject_path }
  → get_python_skill_runtime_info (skills/mod.rs:142)
  → build_rlm_bootstrap_code (rlm_bootstrap.rs:65)
  → kernel namespace: import <module>; callable wrapper (__call__ → run)
```
**Когда**: Python-логика, доступная в REPL (`await edit(...)`).
**Ограничение**: только Python; не виден модели как native tool.

### 8.4 Детали механики C (host_request)
```
Python: await host_request("lsp.did_change", {...})  (rlm/__init__.py:153)
  → repl.host_request({**payload, "type": request_type})
  → Rust: HostRequestHandlers (kernel/shared.rs:270)
  → handlers.register("kind", host_handler(fn))  (shared.rs:280)
  → extra_host_handlers seam (engine.rs:42 → lifecycle.rs:802)
```
**Когда**: Python-код должен вызвать Rust-функцию (мост).
**Пример**: новый `lsp.did_change` из edit-скилла.

### 8.5 Выбор механики для наших фишек

| Фишка | Механика | Почему |
|---|---|---|
| `lsp` tool | **A** (нативный) | модель должна видеть как tool |
| `graph_search`/`trace`/`changes` | **A** | нативные инструменты |
| edit-гибрид | **A + B** | нативный Rust-хост + Python-фасад (совместимость) |
| LSP после edit | **C** | edit (Python) → Rust LSP |
| pa-graph индексация | **A** (фон) | Rust, запуск из create_session |
| ADR | **A** или **C** | tool или host_request |

### 8.6 КРИТИЧНО для E1 (edit-гибрид)
Текущий `edit` — **Python-скилл (механика B)**. Модель вызывает его как
`await edit(...)` в REPL (программный инструмент, НЕ native tool).

Варианты гибрида:
- **E1a**: новый **нативный Rust-tool `edit`** (механика A) — заменяет skill.
  Модель видит `edit` как tool. Python-скилл удаляется/остаётся алиасом.
- **E1b**: сохранить Python-скилл, добавить **host_request** `edit.*` (C)
  → Rust pi-edit. Совместимо (модель не меняет вызов), но двойной хоп.
- **E1c**: нативный tool + Python-фасад (A+B) — оба пути работают.

**Рекомендация (вариант 3, ваш выбор)**: **E1c** — нативный Rust `edit`
(exact-match по умолчанию + опц. AST/hashline), Python-фасад сохранён для
совместимости (upstream-сессии продолжают работать).

### 8.7 Prompt-стоимость каждой механики
| Механика | Prompt-стоимость |
|---|---|
| A (native tool) | ~50-150 токенов (JSON Schema + description) |
| B (Python skill) | ~0 (SKILL.md в динамическом tail) |
| C (host_request) | ~0 (не виден модели напрямую) |
| D (MCP) | ~0-100 (mcp-guidance) |

**Следствие**: нативные tools (A) дороже по промпту → минимизировать число,
давать компактные описания. edit-гибрид (E1c): нативный tool добавит
~100 токенов (приемлемо).

---

## 9. ОБНОВЛЁННЫЕ ОСИ (с учётом механик)

### 9.1 Ось «механика» (какая механика у фишки)
| Значение | Смысл |
|---|---|
| `native` | Rust AgentTool (A) |
| `skill` | Python-skill (B) |
| `host` | host_request (C) |
| `mcp` | MCP server (D) |
| `native+skill` | гибрид (A+B) |

### 9.2 Ось «видимость модели»
| Значение | Смысл |
|---|---|
| `tool` | модель видит как tool (wire) |
| `repl` | модель вызывает в Python REPL |
| `both` | и tool, и REPL |
| `hidden` | внутренний (не виден) |

### 9.3 Ось «синхронность»
| Значение | Смысл |
|---|---|
| `sync` | блокирует turn |
| `async` | фон (уведомление о завершении) |
| `lazy` | по запросу |
| `eager` | при старте сессии |

### 9.4 Ось «персистентность»
| Значение | Смысл |
|---|---|
| `session` | в памяти сессии |
| `project` | .db на проект |
| `global` | ~/.allspark |
| `none` | эфемерно |

---

## 10. ПОЛНАЯ МАТРИЦА (фишка × механика × видимость × синхронность × персистентность)

| Фишка | Механика | Видимость | Синхр. | Персист. | Ось-флаг |
|---|---|---|---|---|---|
| L1 LSP-клиент | native (внутр.) | hidden | lazy | session | lsp.enabled |
| L2 lsp tool | native | tool | async | session | lsp.actions |
| L3 name_path | native (внутр.) | hidden | sync | none | — |
| L4 diagnostics | native+host | both | sync | session | lsp.diagnostics |
| L5 rename | native | tool | sync | project | lsp.rename |
| L6 refs/def/hover | native | tool | sync | session | — |
| L11 LSP-surface | native | hidden | async | project | lsp.surface |
| G1 SQLite-граф | native | hidden | — | project | graph.enabled |
| G2 индексация | native | hidden | async | project | graph.index |
| G3 graph_search | native | tool | sync | project | graph.search |
| G4 graph_trace | native | tool | sync | project | graph.trace |
| G5 graph_changes | native | tool | sync | project | graph.changes |
| G6 graph_arch | native | tool | sync | project | graph.arch |
| G7 closure-repair | native | hidden | async | project | graph.incremental |
| G10 ADR | native | tool | sync | project | graph.adr |
| V1 FTS5 | native (внутр.) | hidden | — | project | graph.fts |
| V2 BM25+RRF | native | hidden | sync | project | graph.hybrid |
| V3 эмбеддинги | native | hidden | async | project | graph.semantic |
| V5 RaBitQ | native | hidden | sync | project | graph.rabitq |
| M1 CoherencePolicy | native | hidden | async | session | graph.compaction |
| M2 temporal decay | native | hidden | sync | session | graph.decay |
| E1 edit-гибрид | native+skill | both | sync | none | edit.mode |
| E2 AST-правки | native | tool | sync | none | edit.ast |
| E3-E5 guards | native (внутр.) | hidden | sync | session | edit.guards |
| E6 diagnostics-after-edit | host | hidden | async | session | edit.diagnostics |
| P1-P6 RAM-граф | native | hidden | — | session | graph.shards |

---

## 11. ЧТО ЕЩЁ ДОБАВИТЬ (сверх исходного плана)

### 11.1 Инструменты контроля (новые)
| Tool | Зачем | Механика |
|---|---|---|
| `graph_status` | generation/coverage/размер | native |
| `graph_coverage` | честность индекса | native |
| `lsp_status` | живые серверы | native |
| `lsp_capabilities` | что умеет сервер | native |
| `graph_doctor` | диагностика .db/индекса | native |

### 11.2 Хуки жизненного цикла (новые host_request)
| Hook | Триггер | Механика |
|---|---|---|
| `lsp.did_change` | edit сохранил файл | host |
| `lsp.did_open` | сессия стартовала | host |
| `graph.invalidate` | файл изменён | host |
| `graph.warm` | первый запрос | host |

### 11.3 Настройки (settings.json, новые)
| Ключ | Тип | Default |
|---|---|---|
| `lsp` | object | {enabled:false} |
| `graph` | object | {enabled:false} |
| `edit` | object | {mode:"exact",guards:true} |

### 11.4 Телеметрия (новые метрики)
| Метрика | Источник |
|---|---|
| `graph_index_ms` | индексация |
| `graph_query_ms` | запросы |
| `lsp_call_ms` | LSP-запросы |
| `edit_guard_hits` | guards |

### 11.5 Предохранители (новые)
| Предохранитель | Защита |
|---|---|
| `graph.maxMemoryMb` | OOM |
| `graph.maxDbMb` | диск |
| `lsp.maxClients` | процессы |
| `graph.indexBudgetPct` | время |
| Fallback на rg/Python | недоступность |

---

## 12. ИТОГ: ПОЛНЫЙ РЕЕСТР (для реализации)

**Всего фишек**: ~45 (L1-L11, G1-G11, V1-V10, M1-M5, E1-E7, P1-P8).
**Механики**: native (большинство), host (LSP-хуки), skill (edit-фасад).
**Новые крейты**: pa-lsp, pa-ast, pa-graph.
**Новые tools**: lsp, graph_search, graph_trace, graph_changes, graph_arch,
graph_status, graph_coverage, lsp_status.
**Новые хуки**: lsp.did_change, graph.invalidate, graph.warm.
**Новые settings**: lsp{}, graph{}, edit{}.
**Оси контроля**: функциональные (15+), операционные (8), изоляции (4),
наблюдаемости (4), механики (5), видимости (4), синхронности (4),
персистентности (4).


---

## 13. ВЕРИФИКАЦИЯ СОВМЕСТИМОСТИ (что сломается / что нет)

### 13.1 Тесты, которые НЕ сломаются
| Тест | Почему безопасно |
|---|---|
| `prompt_guards.rs:85,547` | задаёт `selected_tools: Some(vec!["python_repl"])` явно |
| `golden_replay.rs` | работает с `create_bash/edit_tool_definition` напрямую |
| engine/tests.rs | проверяет конкретные сценарии, не число tools |

**Вывод**: добавление native tools в `config.tools` не сломает существующие
тесты, потому что они либо задают tools явно, либо не проверяют их число.

### 13.2 Что НУЖНО обновить при добавлении tool
| Место | Изменение |
|---|---|
| `core.md` | секция про native tools (документация) |
| `system_prompt.rs` | ветвление по имени (шаблон `has_ipython`) |
| `settings/types.rs` | поля `lsp`, `graph`, `edit` |
| `lifecycle.rs:1022` | прокинуть managers |
| `engine.rs:446` | push tools (по флагу) |

### 13.3 Parity с TS (важно!)
prime-agent — порт TS-продукта. Многие тесты сверяют байт-в-байт с TS.
| Аспект | Риск | Митигация |
|---|---|---|
| Prompt text | TS не имеет lsp/graph → расхождение | фича-флаг default-off |
| Tool schemas | TS не отправляет lsp → расхождение | только при флаге |
| Session files | новые поля → расхождение | не писать в session header |
| Settings | новые ключи → TS игнорирует | additive keys |

**Ключ**: фича-флаги default-off → при выключенных флагах prime-agent
**байт-идентичен** upstream (parity-тесты проходят).

### 13.4 Изоляция от upstream (merge)
| Файл | Вторжение | Маркер |
|---|---|---|
| `engine.rs` | 1 блок push tools | FORK-ONLY |
| `lifecycle.rs` | 1 вызов manager | FORK-ONLY |
| `core.md` | 1 секция | FORK-ONLY |
| `system_prompt.rs` | 1 ветвление | FORK-ONLY |
| `settings/types.rs` | 3 поля | FORK-ONLY |
| новые крейты | целиком | FORK-ONLY в Cargo.toml |

---

## 14. МАТРИЦА «ЧТО ДАЁТ КАЖДАЯ ФИШКА» (ценность × стоимость)

| Фишка | Ценность | Стоимость | Приоритет |
|---|---|---|---|
| G3 graph_search | ★★★ | средняя | P0 |
| G4 graph_trace | ★★★ | низкая | P0 |
| L4 diagnostics | ★★★ | средняя | P0 |
| G7 closure-repair | ★★★ | высокая | P1 |
| L2 lsp tool | ★★★ | средняя | P0 |
| G5 graph_changes | ★★ | низкая | P1 |
| E6 diagnostics-after-edit | ★★★ | низкая | P1 |
| V2 BM25+RRF | ★★ | низкая (готовый) | P1 |
| L5 rename | ★★ | низкая | P1 |
| G10 ADR | ★★ | низкая | P2 |
| M1 CoherencePolicy | ★★ | низкая (готовый) | P2 |
| G2 индексация | ★★★ | высокая | P0 (базис) |
| V5 RaBitQ | ★ | высокая (safe) | P3 |
| V7 nomic | ★ | низкая (+31MB) | P3 |
| P1-P6 RAM-граф | ★ | средняя | P3 |
| V8 ACORN | ★ | низкая | P3 |

### Критический путь (минимальный MVP)
```
G2 (индексация) → G1 (SQLite) → G3 (search) → G4 (trace)
L1 (LSP-клиент) → L2 (lsp tool) → L4 (diagnostics)
E6 (diagnostics-after-edit)
```
Это даёт **80% ценности** за **~30% работы**.


---

## 15. ПОЛНЫЙ ИНВЕНТАРЬ SERENA (восполнение пропуска)

Ранее Serena была покрыта только 2 строками (L3, L9). На самом деле это
**49 tools + 26 contexts + 9 modes + 8 facades**. Ниже — полный разбор.

### 15.1 Tool-классы Serena (49, по модулям)

**symbol_tools.py (13) — символьные операции (LSP):**
| Tool | Аналог в нашей матрице |
|---|---|
| `GetSymbolsOverviewTool` | G6/graph_arch (обзор символов) |
| `FindSymbolTool` | G3/graph_search + name_path |
| `FindReferencingSymbolsTool` | L6 + G4 (references) |
| `FindImplementationsTool` | L6 (implementation) |
| `FindDeclarationTool` | L6 (declaration) |
| `GetDiagnosticsForFileTool` | L4 |
| `GetDiagnosticsForSymbolTool` | **L9** (diagnostics-to-symbol) |
| `ReplaceSymbolBodyTool` | **E2** (AST-правка символа) |
| `InsertAfterSymbolTool` | **E2** (вставка после символа) |
| `InsertBeforeSymbolTool` | **E2** (вставка перед символом) |
| `RenameSymbolTool` | L5 |
| `SafeDeleteSymbol` | **НОВОЕ** (безопасное удаление символа) |
| `RestartLanguageServerTool` | **НОВОЕ** (рестарт LSP) |

**file_tools.py (10) — файловые операции:**
| Tool | Аналог |
|---|---|
| `ReadFileTool` | Python open() |
| `CreateTextFileTool` | write |
| `ListDirTool` | os.listdir |
| `FindFileTool` | glob |
| `ReplaceContentTool` | **E1** (regex/literal replace) |
| `ReplaceInFilesTool` | **НОВОЕ** (batch replace по файлам, dry_run) |
| `DeleteLinesTool` | НОВОЕ (line-based) |
| `ReplaceLinesTool` | НОВОЕ (line-based) |
| `InsertAtLineTool` | НОВОЕ (line-based) |
| `SearchForPatternTool` | rg |

**memory_tools.py (6) — память (дубль continual harness):**
`WriteMemory`, `ReadMemory`, `ListMemories`, `DeleteMemory`,
`RenameMemory` (с обновлением `mem:`-ссылок), `EditMemory` (regex/literal).

**config_tools.py (4):** `OpenDashboard`, `ActivateProject`,
`RemoveProject`, `GetCurrentConfig`.

**query_project_tools.py (2):** `ListQueryableProjects`, `QueryProject`
(запрос к indexed-проектам).

**cmd_tools.py (1):** `ExecuteShellCommand`.

**repl_tools.py (1):** `SerenaReplTool` — **Python REPL с facades** (аналог
нашего python_repl!).

**workflow_tools.py (3):** `Onboarding`, `InitialInstructions`, `SerenaInfo`.

**jetbrains_tools.py (12):** JetBrains-бэкенд — **НЕ переносим** (нам LSP).

### 15.2 Contexts Serena (26) — КОНФИГУРАЦИЯ TOOL SURFACE ⭐

**Это ключевое, что я пропустил.** Context = профиль tool surface для
конкретного клиента/среды. Схема (`context.template.yml`):
```yaml
description: ...
prompt: |              # добавляется в system prompt
excluded_tools: []     # исключить tools
included_optional_tools: []  # включить опциональные
fixed_tools: []        # заменить весь набор
excluded_apis: []      # исключить facade-методы из REPL
included_apis: []
tool_description_overrides: {}  # переопределить описания (для ChatGPT)
single_project: false  # ограничить одним проектом
structured_tool_output: null
```

**26 контекстов**: `ide`, `codex`, `claude-code`, `chatgpt`, `grok`, `agent`,
`copilot-cli`, `vscode`, `codebuddy`, `desktop-app`, `oaicompat-agent`,
`jb-ai-assistant`, `antigravity`, `junie`, `zcode`, `jb-copilot-plugin`...

Пример `codex.yml`: исключает file/shell tools (Codex делает это сам),
оставляет символьные. Prompt объясняет: «не читай файлы целиком, используй
symbolic tools».

**ПЕРЕНОС В PRIME**: это **точная аналогия** нашей идеи «фича-флаги
lsp.enabled/graph.enabled/edit.mode»! Serena формализует это как контексты:
- Профиль «codex» → нативный python_repl (не мешать)
- Профиль «symbolic» → lsp/graph tools
- Профиль «planning» → read-only (без edit)
- Профиль «one-shot» → всё + auto-approve

**Новая ось**: `context` (профиль tool surface).

### 15.3 Modes Serena (9) — РЕЖИМЫ РАБОТЫ ⭐

Mode = подмножество/надстройка контекста, активируется динамически.
- `editing.yml` — все tools + инструкции по правкам (символьные > файловые).
- `planning.yml` — только read-only (исключены все правки).
- `one-shot.yml` — автономная работа, auto-approve.
- `interactive.yml`, `onboarding.yml`, `no-onboarding.yml`,
  `no-memories.yml`, `query-projects.yml`, `benchmark.yml`.

**Ключевая идея**: правки через **символьные tools** предпочтительнее файловых
(`rename_symbol`, `replace_symbol_body` — reference-aware, atomic).
Prompt: «trust it: do not re-read the changed files».

**ПЕРЕНОС В PRIME**: mode ≈ наш `autonomous`/`planning` + новый
`edit.symbolic` (предпочитать символьные правки).

### 15.4 Facades Serena (8) — REPL-API ⭐

Serena's REPL (`s` объект) даёт **facades**: `lsp`, `edit`, `fs`, `mem`,
`shell`, `cfg`, `ext`, `jb`. Каждая — объект с методами.
```
s.lsp.find_symbol(name_path, relative_path)
s.edit.replace_content(...)
s.info("lsp")  # документация facade
```
**ПЕРЕНОС В PRIME**: наш python_repl может получить `s.lsp`/`s.graph`
facades → программный доступ к графу/LSP из Python (как `mcp` сегодня).

### 15.5 Prompt-templates Serena ⭐
`resources/config/prompt_templates/` — шаблоны промптов для контекстов/modes.
`{{ tool_names['rename_symbol'] }}` — подстановка имён tools.

---

## 16. ОБНОВЛЁННАЯ МАТРИЦА: Serena-фишки (расширенно)

### 16.1 Новые строки (Serena, которых НЕ было)

| # | Фишка | Сейчас | Как | Механика | Пересечения | Ось |
|---|---|---|---|---|---|---|
| S1 | **Contexts** (26 профилей) | нет | Rust (settings-профили) | native | system_prompt, tools | `context` |
| S2 | **Modes** (9 режимов) | autonomous/planning | Rust (settings) | native | context | `mode` |
| S3 | **Facades** (s.lsp, s.edit...) | mcp object | Python+host | host | python_repl | `facades` |
| S4 | `replace_in_files` (batch, dry_run) | нет | Rust | native tool | edit | `edit.batch` |
| S5 | `replace_content` (regex/literal) | exact-match | Rust edit-режим | native | E1 | `edit.mode` |
| S6 | `replace_symbol_body` | нет | Rust (pa-ast+LSP) | native tool | E2, G2 | `edit.symbolic` |
| S7 | `insert_before/after_symbol` | нет | Rust (pa-ast) | native tool | E2 | `edit.symbolic` |
| S8 | `safe_delete_symbol` | нет | Rust (LSP refs) | native tool | L6 | `edit.symbolic` |
| S9 | `get_symbols_overview` (depth) | нет | Rust (pa-ast) | native tool | G6 | `graph.overview` |
| S10 | `find_declaration` | нет | Rust LSP | native tool | L6 | — |
| S11 | `find_implementations` | нет | Rust LSP | native tool | L6 | — |
| S12 | `get_diagnostics_for_symbol` | нет | Rust (L9) | native tool | L4, L9 | `lsp.symbolDiag` |
| S13 | `restart_language_server` | нет | Rust LSP | native tool | L1 | `lsp.restart` |
| S14 | `search_for_pattern` | rg | Rust (graph regex) | native tool | G3 | `graph.pattern` |
| S15 | `query_project` | нет | Rust | native tool | G1 | `graph.query` |
| S16 | `tool_description_overrides` | нет | Rust (settings) | native | system_prompt | `prompt.overrides` |
| S17 | `single_project` | нет | Rust (settings) | native | — | `graph.scope` |
| S18 | `structured_tool_output` | нет | Rust (settings) | native | — | `output.format` |
| S19 | `excluded_apis`/`included_apis` | нет | Rust (settings) | native | facades | `facades.filter` |
| S20 | `initial_instructions`/`onboarding` | harness | Rust (prompt) | native | prompt | `onboarding` |

### 16.2 Соответствие Serena ↔ наша матрица

| Serena | Наша фишка | Статус |
|---|---|---|
| symbol_tools (13) | L4,L5,L6,L9,E2 | частично покрыто |
| **contexts (26)** | **S1 (НОВОЕ)** | пропущено! |
| **modes (9)** | **S2 (НОВОЕ)** | пропущено! |
| **facades (8)** | **S3 (НОВОЕ)** | пропущено! |
| file_tools (10) | E1, E7, rg | частично |
| memory_tools (6) | continual harness | дубль (не переносим) |
| jetbrains (12) | — | не переносим |
| repl_tool (1) | python_repl | уже есть |

### 16.3 КРИТИЧЕСКИЙ ВЫВОД
Serena даёт **систему конфигурации tool surface** (contexts+modes+facades),
которой в нашей матрице НЕ было. Это прямо решает нашу задачу изоляции:
- **context** = профиль (codex/symbolic/planning/one-shot)
- **mode** = динамический режим
- **facade** = REPL-доступ

**Новые оси**:
- `context` (26 профилей → адаптация под клиента)
- `mode` (9 режимов → editing/planning/one-shot)
- `facades` (lsp/edit/graph в REPL)
- `edit.symbolic` (предпочитать символьные правки)
- `prompt.overrides` (переопределение описаний)

Это делает дизайн **богаче**: не просто «фича-флаги», а **профили** (как
Serena contexts) — наборы tools под задачу/клиента.

---

## 17. ОБНОВЛЁННЫЙ РЕЕСТР ФИШЕК

**Было**: ~45 фишек (L1-L11, G1-G11, V1-V10, M1-M5, E1-E7, P1-P8).
**Стало**: ~65 фишек (+20 Serena: S1-S20).

**Новые категории**:
- **Конфигурация** (S1-S3, S16-S19): contexts, modes, facades, overrides.
- **Символьные правки** (S4-S9): replace_symbol_body, insert_*, safe_delete.
- **Расширенные LSP** (S10-S13): declaration, implementation, symbol-diag, restart.
- **Поиск** (S14-S15): pattern, query_project.

**Новые оси**:
- `context` (профиль)
- `mode` (режим)
- `facades` (REPL-API)
- `edit.symbolic` (тип правок)
- `prompt.overrides`
- `output.format` (structured)
- `onboarding`


---

## 18. ЕЩЁ ДВА ПРОПУЩЕННЫХ ПРИЁМА SERENA

### 18.1 Прогрессивное раскрытие документации (`s.info()`) ⭐
Serena НЕ грузит всю документацию в промпт. Вместо этого:
- `s.info()` — обзор facades (~компактно).
- `s.info("lsp")` — документация facade lsp (по запросу).
- `s.info("lsp.find_symbol")` — конкретный метод.
- `s.info("LspSymbolCollection")` — тип результата.

**Идея**: промпт содержит МИНИМУМ; детали — по требованию. Экономия токенов
при большом tool surface.

**ПЕРЕНОС**: наш python_repl может получить `graph.info()`/`lsp.info()` —
прогрессивное раскрытие для наших tools. Особенно важно при 20+ tools.

### 18.2 Tool-маркеры (классификация) ⭐
Serena помечает каждый tool маркерами (`tools_base.py:71-105`):
| Маркер | Смысл |
|---|---|
| `ToolMarkerCanEdit` | может менять файлы |
| `ToolMarkerSymbolicRead` | символьное чтение |
| `ToolMarkerSymbolicEdit` | символьная правка |
| `ToolMarkerOptional` | отключён по умолчанию |
| `ToolMarkerBeta` | нестабильный |
| `ToolMarkerDoesNotRequireActiveProject` | работает без проекта |

**Идея**: маркеры → контексты фильтруют tools по маркерам (planning
исключает все CanEdit; codex исключает файловые).

**ПЕРЕНОС**: наши tools должны нести метаданные:
- `readonly` (planning-safe)
- `requires_project` (нужен индекс)
- `optional` (по флагу)
- `beta`
- `edit_class` (exact/symbolic/file)

Это даёт **автоматическую фильтрацию** по контексту/mode.

### 18.3 Итог: Serena как система конфигурации
Serena — не просто набор tools, а **фреймворк**:
```
Tool (49) + маркеры
  → Context (26): профиль под клиента (excluded/included/fixed tools, prompt)
  → Mode (9): динамический режим (editing/planning/one-shot)
  → Facade (8): REPL-API (s.lsp, s.edit)
  → s.info(): прогрессивное раскрытие
  → prompt_templates: шаблоны с {{ tool_names }}
```

**Это самое ценное у Serena** для нашего дизайна — не отдельные tools,
а **система управления tool surface**. Она прямо решает:
1. Изоляцию от upstream (context «upstream» = без наших tools).
2. Адаптацию под клиента (codex vs chatgpt vs ide).
3. Экономию токенов (s.info вместо полного промпта).
4. Безопасность (planning = read-only).

---

## 19. ФИНАЛЬНЫЙ РЕЕСТР (полный, с Serena)

| Категория | Кол-во | Источник |
|---|---|---|
| LSP/семантика (L) | 11 | oh-my-pi + Serena |
| Граф (G) | 11 | codebase-memory |
| Поиск/векторы (V) | 10 | codebase-memory + ruvector |
| Память (M) | 5 | ruvector + pogocache |
| Edit (E) | 7 | oh-my-pi + Serena |
| RAM/CPU (P) | 8 | pogocache |
| **Serena-конфиг (S)** | **20** | **Serena (НОВОЕ)** |
| **ИТОГО** | **~72 фишки** | 5 источников |

### Полный набор осей контроля
| Класс | Оси |
|---|---|
| Функциональные | lsp.enabled, graph.enabled, edit.mode, +15 |
| Операционные | graph.maxMemoryMb, lsp.idleTimeoutMs, +8 |
| Изоляции | graph.dbPath, graph.scope, +4 |
| Наблюдаемости | graph.trace, graph.stats, +4 |
| Механики | native/skill/host/mcp |
| Видимости | tool/repl/both/hidden |
| Синхронности | sync/async/lazy/eager |
| Персистентности | session/project/global/none |
| **Конфигурации (НОВОЕ)** | **context, mode, facades, prompt.overrides** |
| **Классификации (НОВОЕ)** | **readonly, requires_project, optional, beta, edit_class** |

### Что даёт Serena-конфигурация для prime
1. **Context «upstream»**: без наших tools → байт-идентичность.
2. **Context «symbolic»**: lsp/graph tools → максимум возможностей.
3. **Mode «planning»**: read-only → безопасный анализ.
4. **Mode «one-shot»**: автономность + auto-approve.
5. **Facades**: `s.lsp`, `s.graph` в python_repl.
6. **s.info()**: прогрессивное раскрытие (экономия токенов).
7. **Маркеры**: авт. фильтрация по контексту.


---

## 20. ДОПОЛНЕНИЕ: неразобранные крейты ruvector (перепроверка)

Ранее я покрыл 11 крейтов ruvector. На самом деле релевантных — **~40**.
Ниже — важные пропущенные.

### 20.1 Кэш и производительность

| Крейт | LOC | Приём | Переносимость |
|---|---|---|---|
| **ruvector-query-cache** | 1 122 | **Semantic Query Cache**: если новый запрос близок (cosine) к ранее отвеченному → вернуть кэш. 3 варианта: NoCache/ExactCache/SemanticCache | ⭐⭐⭐ для graph_search |
| **ruvector-turboquant** | 1 857 | **Turbo4**: 4-bit Lloyd-Max datatype (ADR-296), Hadamard rotation (SplitMix64, bit-stable), nibble-коды `D/2+8` байт (≈7.9× vs f32), **оригинал НЕ хранится** | ⭐⭐ (альтернатива RaBitQ) |
| **ruvector-turbovec** | 1 528 | turbo-векторы | ⭐ |
| **ruvector-speculative-ann** | 1 394 | **Speculative ANN**: быстрый draft (u8) предлагает кандидатов → exact f32 верифицирует; адаптивный контроллер держит recall | ⭐⭐ |
| **ruvector-adaptive-ann** | 1 366 | **Recall-targeted ANN**: задаёшь recall target (0.95), движок сам выбирает min beam width | ⭐⭐ |
| **ruvector-entropy-ann** | 1 401 | **Entropy-adaptive beam search** (arXiv:2505.15636) | ⭐ |
| **ruvector-spann** | 1 084 | **SPANN**: partition spilling для boundary-safe ANN | ⭐ |
| **ruvector-matryoshka** | 1 147 | (уже был) coarse→fine | ⭐⭐ |
| **ruvector-recall-bounded** | 905 | **Recall-bounded search**: возвращает всё выше порога θ с измеренным recall (не фикс. k) | ⭐⭐ |
| **ruvector-hnsw-repair** | 1 222 | **Online HNSW repair** после удалений: Tombstone/BatchRepair/EagerRepair | ⭐ |

### 20.2 Граф и структуры

| Крейт | LOC | Приём | Переносимость |
|---|---|---|---|
| **ruvector-graph** | 25 227 | **Граф-БД** с Neo4j-совместимостью: property graphs, **гиперграфы**, **Cypher**, ACID, distributed | ⭐⭐⭐ (эталон для pa-graph) |
| **ruvector-graph-condense** | 4 282 | **Structure-preserving condensation** через dynamic min-cut (не GCond — детерминированно, без обучения) | ⭐⭐ |
| **ruvector-mincut** | 55 822 | **Subpolynomial dynamic min-cut** (edge insert/delete) | ⭐ (тяжело) |
| **ruvector-dag** | 15 448 | DAG для query-планов + 7 attention-механизмов + SONA MicroLoRA | ⭐ (специфично) |
| **ruvector-namespace-merge** | 1 214 | **S-T mincut namespace routing** для multi-namespace памяти | ⭐⭐ (для subagents) |

### 20.3 Изоляция и governance

| Крейт | LOC | Приём | Переносимость |
|---|---|---|---|
| **ruvector-context** | 2 705 | **ScopedContextIndex**: физическая изоляция по scope (не metadata-фильтр!), авторизованные descendant shards ДО ANN | ⭐⭐⭐ (для субагентов/проектов) |
| **ruvector-collections** | 2 572 | Мульти-коллекции + **алиасы** + статистика | ⭐⭐ |
| **ruvector-capgated** | 1 080 | **Capability-gated retrieval**: 64-бит read-masks внутри поиска | ⭐⭐ |
| **ruvector-filter** | 1 505 | **Payload indexing + фильтры**: equality/range/geo/text/logical (AND/OR/NOT) | ⭐⭐ |
| **ruvector-acorn** | 1 080 | (уже был) predicate-aware ANN | ⭐⭐ |

### 20.4 Provenance / audit / verification

| Крейт | LOC | Приём | Переносимость |
|---|---|---|---|
| **ruvector-retrieval-receipt** | 1 050 | **Witness-chained receipts** для read-path: результат запроса → receipt, нельзя изменить без провала верификации | ⭐⭐ (audit графа) |
| **ruvector-verified** | 2 305 | **Формальная верификация** (lean-agentic dependent types): proof-carrying operations | ⭐ (тяжело) |
| **ruvector-proof-gate** | 1 262 | Proof-gate для write-path | ⭐ |
| **ruvector-staged-workspace** | 1 658 | **StagedWorkspace** (arXiv:2608.18050): content-hash + revision-id на каждый артефакт при записи | ⭐⭐⭐ (для edit/graph) |
| **ruvector-snapshot** | 844 | (уже был) снапшоты + checksums | ⭐⭐ |

### 20.5 Компрессия / сжатие

| Крейт | LOC | Приём | Переносимость |
|---|---|---|---|
| **ruvector-temporal-tensor** | 15 478 | (уже был) низкобитные временные кодеки | ⭐⭐ |
| **ruvector-pq-search** | 1 180 | (уже был) product quantization | ⭐⭐ |
| **ruvector-rabitq** | 3 837 | (уже был) 1-bit | ⭐⭐ (unsafe!) |
| **ruvector-sparsifier** | 2 742 | Разрежение векторов | ⭐ |
| **ruvector-dither** | — | Pre-quantization dithering для low-bit | ⭐ |
| **ruvector-gnn-rerank** | 1 409 | **GNN reranking**: score diffusion по k-NN подграфу восстанавливает recall | ⭐⭐ |

### 20.6 Наблюдаемость / метрики

| Крейт | LOC | Приём | Переносимость |
|---|---|---|---|
| **ruvector-metrics** | 494 | **Prometheus-метрики**: counters/gauges/histograms + health/readiness | ⭐⭐ |
| **ruvector-profiler** | 673 | Профайлер | ⭐ |
| **ruvector-coherence** | 1 340 | **Метрики coherence**: сравнение attention-механизмов, quality guardrails | ⭐ |

### 20.7 Модели / обучение (специфично, не для нас)

| Крейт | LOC | Приём |
|---|---|---|
| ruvector-nervous-system | 27 296 | Био-инспирированные компоненты (dendritic, HDC, cognitive routing) |
| ruvector-solver | 14 543 | Итеративные sparse linear solvers (Neumann, Jacobi...) |
| ruvector-sparse-inference | 10 093 | PowerInfer-style sparse inference для LLM |
| ruvector-attention | — | Attention-механизмы (mincut-gated, hyperbolic...) |
| ruvllm | — | LLM runtime |
| sona | — | Self-Organizing Neural Architecture (MicroLoRA, EWC++) |
| ruvector-gnn | — | Graph neural networks |

---

## 21. ОБНОВЛЁННАЯ МАТРИЦА: ruvector-дополнение

| # | Фишка | Сейчас | Как | Механика | Пересечения | Ось |
|---|---|---|---|---|---|---|
| R1 | Semantic Query Cache | нет | Rust ruvector-query-cache | native | G3, V2 | `graph.cache` |
| R2 | Turbo4 4-bit datatype | нет | Rust ruvector-turboquant | native | V5 | `graph.quant` |
| R3 | Speculative ANN | нет | Rust ruvector-speculative-ann | native | V5 | `graph.speculative` |
| R4 | Recall-targeted ANN | нет | Rust ruvector-adaptive-ann | native | G3 | `graph.recallTarget` |
| R5 | Recall-bounded search | нет | Rust ruvector-recall-bounded | native | G3 | `graph.recallBound` |
| R6 | HNSW repair (deletions) | нет | Rust ruvector-hnsw-repair | native | G1 | `graph.repair` |
| R7 | Graph DB (Cypher, гиперграфы) | нет | Rust ruvector-graph (эталон) | native | G1 | `graph.cypher` |
| R8 | Graph condensation | нет | Rust ruvector-graph-condense | native | G1 | `graph.condense` |
| R9 | Namespace-merge (mincut) | нет | Rust ruvector-namespace-merge | native | субагенты | `graph.namespaces` |
| R10 | ScopedContextIndex (физ. изоляция) | нет | Rust ruvector-context | native | G1 | `graph.scopes` |
| R11 | Collections + aliases | нет | Rust ruvector-collections | native | G1 | `graph.collections` |
| R12 | Capability-gated (64-bit masks) | нет | Rust ruvector-capgated | native | R10 | `graph.capabilities` |
| R13 | Payload filters (range/text/logical) | нет | Rust ruvector-filter | native | G3, V8 | `graph.filters` |
| R14 | Retrieval receipts (read audit) | нет | Rust ruvector-retrieval-receipt | native | M4 | `graph.receipts` |
| R15 | StagedWorkspace (hash+revision) | нет | Rust ruvector-staged-workspace | native | E1, G7 | `edit.staged` |
| R16 | GNN reranking | нет | Rust ruvector-gnn-rerank | native | V3 | `graph.rerank` |
| R17 | Prometheus metrics | нет | Rust ruvector-metrics | native | телеметрия | `graph.metrics` |
| R18 | Coherence metrics | нет | Rust ruvector-coherence | native | M1 | `graph.coherence` |

### 21.1 Ключевые открытия (перепроверка)

1. **Semantic Query Cache** (R1) — прямой выигрыш для `graph_search`:
   агенты задают похожие запросы → кэш экономит ANN.
2. **ScopedContextIndex** (R10) — **физическая** изоляция по scope, не
   metadata-фильтр. Идеально для субагентов (каждый scope = свой индекс).
3. **StagedWorkspace** (R15) — content-hash + revision-id на артефакт →
   решает stale-tag проблему edit (E3)!
4. **Recall-targeted/bounded ANN** (R4, R5) — вместо тюнинга `ef`, задаёшь
   recall → движок подбирает. Для graph_search: контроль качества.
5. **ruvector-graph** (R7, 25K LOC) — **готовый граф-движок с Cypher**!
   Ранее я писал «Cypher не переносить» — но тут он готов.
6. **Payload filters** (R13) — range/text/logical для поиска символов
   (найти функцию с параметром X в диапазоне строк Y).
7. **Retrieval receipts** (R14) — audit read-path (для отладки/трассировки).
8. **Prometheus metrics** (R17) — готовая телеметрия.

### 21.2 Пересмотр решения «Cypher не переносить»
Ранее: «Cypher-движок codebase-memory (5 939 LOC C) — слишком дорого».
Теперь: **ruvector-graph (25 227 LOC Rust) даёт Cypher готовым**.
→ **Пересмотр**: если нужен Cypher, брать `ruvector-graph`, не писать с нуля.

### 21.3 Пересмотр «RaBitQ vs альтернативы»
Появились: Turbo4 (4-bit, R2), Speculative (R3), Adaptive/Entropy/Recall-bounded
(R4-R5). Выбор квантизации шире, чем думали.

---

## 22. ФИНАЛЬНЫЙ РЕЕСТР (после перепроверки ruvector)

| Категория | Кол-во | Источник |
|---|---|---|
| L (LSP/семантика) | 11 | oh-my-pi + Serena |
| G (граф) | 11 | codebase-memory |
| V (поиск/векторы) | 10 | codebase-memory + ruvector |
| M (память) | 5 | ruvector + pogocache |
| E (edit) | 7 | oh-my-pi + Serena |
| P (RAM/CPU) | 8 | pogocache |
| S (Serena-конфиг) | 20 | Serena |
| **R (ruvector-доп)** | **18** | **ruvector (НОВОЕ)** |
| **ИТОГО** | **~90 фишек** | 5 источников |

### Новые оси (ruvector-доп)
- `graph.cache` (semantic query cache)
- `graph.quant` (Turbo4/RaBitQ/PQ)
- `graph.speculative` / `graph.recallTarget` / `graph.recallBound`
- `graph.cypher` / `graph.condense` / `graph.namespaces`
- `graph.scopes` / `graph.collections` / `graph.capabilities`
- `graph.filters` / `graph.receipts` / `graph.rerank`
- `edit.staged` (StagedWorkspace)
- `graph.metrics` / `graph.coherence`


---

## 23. ФИНАЛЬНЫЕ ДОПОЛНЕНИЯ (2-я перепроверка ruvector)

### 23.1 Delta-семейство (инкрементальные изменения) ⭐⭐
| Крейт | LOC | Приём | Переносимость |
|---|---|---|---|
| **ruvector-delta-core** | 3 191 | **Delta/DeltaStream/DeltaWindow**: представление изменения между состояниями, event sourcing, sparse/dense кодирование, delta-компрессия | ⭐⭐⭐ для closure-repair! |
| **ruvector-delta-graph** | 1 567 | **Delta-операции для графа**: edge add/remove/update, node property deltas, delta-aware traversal, batch | ⭐⭐⭐ для pa-graph! |
| **ruvector-delta-index** | 1 636 | Delta-индексы | ⭐⭐ |
| **ruvector-delta-consensus** | 1 636 | Delta-консенсус | ⭐ |

**Ключ**: это **ровно то, что нужно для closure-repair** (G7). Вместо
перестройки — применять delta (edge add/remove). `ruvector-delta-graph`
даёт готовые `EdgeDelta`, `NodeDelta`, `delta-aware traversal`.

### 23.2 Self-Reconstructing Graph Memory (ADR-270) ⭐⭐⭐
Идея MRAgent: **«память реконструируется, а не извлекается»**.
- Traversal вместо flat retrieval.
- **Adaptive cost**: дешёвый обход для тривиального, глубокий для сложного.
- **Calibration**: агент должен знать, когда НЕ знает (abstain).
- **Self-organization**: частые ассоциации укорачиваются (shortcut edges).
- Корпус достиг 100% accuracy при `traversalDepth=3`.

**Перенос**: для `graph_trace` — adaptive depth (не фиксированный);
shortcut edges после успешного обхода.

### 23.3 mcp-brain (shared collective memory) ⭐
10 MCP-tools: `brain_share`, `brain_search`, `brain_get`, `brain_vote`
(Bayesian), `brain_transfer` (cross-domain), `brain_drift`, `brain_partition`
(mincut), `brain_list`, `brain_delete`, `brain_status`.
Хранение: RVF containers + witness chains + Ed25519 + differential privacy.

**Перенос**: для **межсессионного обучения** (общий опыт агентов). Но у нас
есть continual harness → возможно, ADR-совместимо.

### 23.4 Прочие релевантные
| Крейт | Приём | Переносимость |
|---|---|---|
| ruvector-coherence-hnsw | Coherence-gated beam search | ⭐⭐ |
| ruvector-namespace-merge | S-T mincut routing | ⭐⭐ |
| ruvector-cognitive-container | WASM-контейнер + witness chain (graph ingest, mincut, spectral) | ⭐ |
| prime-radiant | Universal coherence engine (sheaf Laplacian) | ⭐ |
| ruvector-domain-expansion | Cross-domain transfer | ⭐ |

---

## 24. ПЕРЕСМОТР КЛЮЧЕВЫХ РЕШЕНИЙ (после 2 перепроверок)

| Было | Стало | Причина |
|---|---|---|
| Cypher не переносить | **взять ruvector-graph** (готовый) | 25K LOC Rust уже есть |
| closure-repair писать с нуля | **взять ruvector-delta-graph** | готовые EdgeDelta/NodeDelta |
| RaBitQ — единственная квантизация | Turbo4/Speculative/Adaptive/PQ | выбор шире |
| graph_trace фикс. depth | **adaptive depth** (ADR-270) | self-reconstructing |
| изоляция через фильтры | **ScopedContextIndex** (физ.) | ruvector-context |
| нет query-кэша | **Semantic Query Cache** | ruvector-query-cache |
| stale-tag проблема | **StagedWorkspace** (hash+revision) | ruvector-staged-workspace |

### 24.1 Критическое переосмысление: delta-граф
**ruvector-delta-graph + ruvector-delta-core** — это готовый механизм
инкрементальных изменений графа. Вместо того чтобы писать closure-repair
с нуля (как я планировал из codebase-memory), можно взять:
- `Delta`/`DeltaStream`/`DeltaWindow` (delta-core).
- `EdgeDelta`/`NodeDelta`/`delta-aware traversal` (delta-graph).
- Поверх этого — surface-хэш логику из codebase-memory.

**Это меняет Фазу 5** (closure-repair): не с нуля, а на delta-крейтах.

### 24.2 Обновлённый критический путь
```
Фаза 1: pa-lsp
Фаза 2: pa-ast
Фаза 3: pa-graph disk (SQLite)
Фаза 4: pa-graph RAM
Фаза 5: closure-repair ← ruvector-delta-graph + surface-хэш
Фаза 6: поиск ← ruvector-hybrid (BM25+RRF) + ruvector-query-cache
Фаза 7: LSP-surface + changes
Фаза 7.5: memory lifecycle ← ruvector-agent-memory + temporal-coherence
Фаза 8: edit-гибрид ← ruvector-staged-workspace (hash+revision)
Фаза 9 (опц.): ruvector-graph (Cypher), Turbo4, ScopedContext
```

---

## 25. ФИНАЛЬНЫЙ РЕЕСТР (после 2 перепроверок)

| Категория | Кол-во |
|---|---|
| L (LSP/семантика) | 11 |
| G (граф) | 11 |
| V (поиск/векторы) | 10 |
| M (память) | 5 |
| E (edit) | 7 |
| P (RAM/CPU) | 8 |
| S (Serena-конфиг) | 20 |
| R (ruvector, 1-я) | 18 |
| **R2 (ruvector, 2-я)** | **12** |
| **ИТОГО** | **~102 фишки** |

### Итоговые новые оси (ruvector-2)
- `graph.delta` (delta-core/delta-graph)
- `graph.adaptiveDepth` (self-reconstructing)
- `graph.shortcuts` (self-organization)
- `graph.coherenceGate` (coherence-hnsw)
- `brain.*` (mcp-brain: share/search/vote/transfer)
- `graph.container` (cognitive-container)

### Вывод перепроверки
ruvector оказался **гораздо богаче**: 90+ релевантных крейтов (не 11).
Ключевые находки:
1. **delta-core/delta-graph** — готовый инкрементальный граф (для closure-repair).
2. **ruvector-graph** — готовый Cypher-движок (пересмотр решения «не переносить»).
3. **Semantic Query Cache** — прямой выигрыш для graph_search.
4. **ScopedContextIndex** — физическая изоляция (для субагентов).
5. **StagedWorkspace** — hash+revision (решает stale-tag).
6. **Self-reconstructing** (ADR-270) — adaptive traversal.
7. **mcp-brain** — shared memory (опционально).
8. Богатый выбор квантизации (Turbo4/RaBitQ/PQ/Speculative/Adaptive).

**Главный урок**: для Rust-форка ruvector — **основной источник готовых
крейтов**, а не только алгоритмов. Почти всё, что codebase-memory/pogocache
делают на C, ruvector даёт на Rust.


---

## 26. ГЛУБОКОЕ ИССЛЕДОВАНИЕ ruvector (субагент, 177 крейтов)

Субагент `ruvector-deep` (deepseek-v4.1-flash:high) прошёл **все 177 крейтов** +
322 ADR. Отчёт: `.research/ruvector-full-inventory.md` (297 строк). Верифицировано.

### 26.1 Статистика
- **139/177 крейтов — полностью safe** (0 unsafe). Только 32 содержат unsafe.
- 7 крейтов явно `#![forbid(unsafe_code)]`: hybrid, maxsim, rairs, context,
  graph-condense, gnn-rerank, perception.
- `ruvector-mincut` — `deny(unsafe_code)` вне wasm (`lib.rs:119`).
- `prime-radiant` — `deny(unsafe_code)`.

### 26.2 НОВЫЕ критичные находки (меняют решения)

**N1. `ruvector-decompiler` (7 495 LOC) — конвейер для графа кода** ⭐⭐⭐
```
parse → ReferenceGraph → partition → infer → witness
```
- **Адаптивный алгоритм**: exact MinCut при `<5000` узлов, **Louvain** `O(n log n)`
  (rayon-параллельно) при `≥5000` (`partitioner.rs:3`).
- **`build_module_tree`**: inter-module edge density → агломеративная кластеризация
  → рекурсивный split до `max_depth`. «Иерархия папок = структура графа».
- **Witness chain** (Merkle) доказывает вывод + V3 source-map VLQ.
- unsafe: **3× `from_utf8_unchecked`** (`parser.rs:246,281,292`) — тривиально safe.

**N2. `ruvector-context` (2 714 LOC) — физическая изоляция** ⭐⭐⭐
ADR-334: **отдельный VectorDB на каждый scope** (не post-filter!).
- `ContextNamespace{authority,tenant,subject_kind,subject_id,collection}`.
- filename = SHA-256(domain-separated, length-delimited); recovery fail-closed.
- `QuarantinedShard` — битые шарды не блокируют, но видны.
- Счётчик `searches` на шард («траверсал без подсчёта не компилируется»).
- `#![forbid(unsafe_code)]`, `missing_docs=deny`.

**N3. `ruvector-rulake` (5 237 LOC) — кэш по witness-хэшу** ⭐⭐⭐
`VectorCache` ключ = SHAKE-256 witness: два бэкенда с одинаковыми
`(data_ref, rotation_seed, rerank_factor, generation)` → один witness → общая запись.
Вторичный pointer-map + GC по refcount. **Лекарство от cache-invalidation drift.**

**N4. `ruvector-staged-workspace` (1 667 LOC) — защита от stale read** ⭐⭐⭐
ADR-318: view привязан к `(content_hash, revision_id)`.
- `WorkspaceError::{StaleView, BindingMismatch, AnchorRejected}` — **hard reject**.
- «No anchor, no transition»: запись через `TransitionAnchor` ДО мутации.
- Коммит идентичных байт = no-op (без bump revision).

**N5. `mcp-gate` (4 690 LOC) — ReCache + VoI-лестница** ⭐⭐⭐
- `SchemaResourceCache`: `ResourceId = SHA-256(canonical JSON)`. Блок компилируется
  один раз на контент → смена порядка тулов не перекомпилирует всё.
- `EscalationLadder` (ADR-337): `stream → детектор → risk → verifier → investigator
  → halt`; правило «investigate when P(violation)×damage > cost» +
  **unconditional-inspection floor** (privilege/network/credentials/destructive).

**N6. `ruvector-tiny-dancer-core::voi` — VoI с закрытой формой** ⭐⭐
`decide`/`value_of_information`/`voi_upper_bound` при `U~N(μ,σ²)`. Покупать оценку
только при положительном ожидании. + conformal `UncertaintyEstimator` + `CircuitBreaker`.

**N7. `ruvector-core::advanced_features` — готовые safe-примитивы** ⭐⭐⭐
- **MMR** (diversity rerank): `λ·sim(q,d) − (1−λ)·max sim(d,selected)`.
- **LSM+Bloom**: `MemTable` L0 + `Segment` L1/L2 + `BloomFilter` (negative O(1)).
- **Sparse+RRF**: `SparseVector` + inverted posting + SPLADE-совместимый.
- **PDX-раскладка**: dimension-major ~2× на скалярном коде («layout is the lever»).
- **Arena** (cache-aligned bump) + **LockFree** (ObjectPool/WorkQueue/Counter).
- **Conformal prediction** + filtered search с авто-выбором pre/post-filter.

**N8. `ruvector-graph` — BM25+RRF+индексы (помимо Cypher)** ⭐⭐⭐
- `bm25.rs:1` + `schema.rs:501` `reciprocal_rank_fusion` (k=60).
- `typed_graph.rs:405` — **tri-modal hybrid** (ANN + BM25 + graph-traversal).
- Индексы: `LabelIndex`/`PropertyIndex`/`AdjacencyIndex`, `BloomFilter`,
  `AdaptiveRadixTree`, `RoaringBitmapIndex`+`DeltaEncodedList`.
- `executor/cache.rs` (LRU+TTL), `stats.rs` (histogram/MCV для cost-based),
  `cypher/optimizer.rs` (predicate pushdown, join reordering).
- unsafe — только в `optimization/` (опционально).

**N9. `ruvector-delta-index` — инкрементальный HNSW** ⭐⭐⭐
`DeltaHnsw` + `IncrementalUpdater` + `GraphRepairer` с
`RepairStrategy{Lazy,Eager,Batched,Adaptive}` + `QualityMonitor`/`estimate_recall`.

**N10. `rvAgent::masking` (ADR-274) — observation masking** ⭐⭐⭐
Замена tool-вывода на placeholder с **dereferenceable recall-id**:
`[read_file output elided: 2847 bytes, recall id tc7]`. `messages` = полный
append-only лог (masking = проекция). `keep_last_observations`=8,
`max_tool_result_bytes`=100k. ADR-274: masking ≥ LLM-summarization при ~½ стоимости.

**N11. `ruvector-agent-memory` — TARL-ledger + CAMA** ⭐⭐⭐
- **`TransactionalLedger`** (TARL, ADR-307): 5 операций (add/ignore/
  revise_outdated_belief/reject_unreliable/defer_for_verification), состояния
  accepted/pending/rejected, «stage → witness → commit» (нет witness — нет мутации).
- **CAMA-arbitration**: N воспоминаний с общим causal_parent = 1 наблюдение.
- **`AtomicObservation`** content-addressed (id=SHA-256), резолвимость causal_parents
  обязательна → отношение всегда DAG.

**N12. `ruvector-bounded-rag` — RAG под token-budget** ⭐⭐⭐
`TopK`/`GraphBfs` (coherence-gate)/`MinCutBounded` (max-flow).

**N13. `rvf::QualityEnvelope` — честная деградация** ⭐⭐
Обязательный тип: `RetrievalQuality` (Full/Partial/LayerAOnly/DegenerateDetected/
BruteForceBudgeted), `ResponseQuality`, `DegradationReason`, `FallbackPath`.

**N14. ПРОБЕЛ: SCC/Tarjan/Kosaraju НЕ реализованы** ⚠️
Есть только топосорт + Kahn-валидация (`ruvector-dag`, `prime-radiant`).
SCC и SCC-condensation придётся писать самим (или `petgraph`, которого нет в lock).

### 26.3 Ключевые пересмотры

| Решение | Пересмотр |
|---|---|
| BM25+RRF брать из ruvector-hybrid | Есть **трижды**; для pa-graph брать **вместе с граф-слоем** (`ruvector-graph`) |
| RaBitQ — единственная компрессия | **`ruvector-turbovec`** (0 unsafe, multi-bit 2/3/4-bit) — safe-альтернатива! |
| Cypher не переносить | `ruvector-graph` даёт **BM25+RRF+индексы+Cypher** одним крейтом |
| closure-repair с нуля | семейство: delta-index + StreamingCondenser + DriftingIndex |
| изоляция — метаданные | `ruvector-context` — **физические шарды** (ADR-334) |
| кэш по логическому ключу | `rulake`+`staged-workspace`+`ReCache` — ключ = content-hash |
| unsafe = rabitq+несколько | 139/177 safe; 7 forbid + mincut/prime-radiant deny |

### 26.4 Топ-10 для переноса (все safe кроме #1)
1. **decompiler** (3 тривиальных unsafe) — конвейер кластеризации кода
2. **hybrid/graph BM25+RRF** (safe)
3. **staged-workspace** (safe) — защита от stale read
4. **rulake::cache + mcp-gate::ReCache** (safe) — кэш по content-hash
5. **ruvector-context** (safe) — физическая изоляция
6. **proof-gate + retrieval-receipt** (safe) — provenance
7. **delta-index + StreamingCondenser + DriftingIndex** (safe) — инкрементальность
8. **core::advanced_features** (MMR/LSM/sparse/PDX, safe)
9. **rvAgent::masking + mcp-gate VoI** (safe)
10. **agent-memory ledger/arbitration** (safe)

### 26.5 Опасности
- **unsafe**: rabitq (19), solver (24), core (95, SIMD под cfg), graph (14, в
  `optimization/`), mincut (22, wasm-only), temporal-tensor (23), turboquant (15).
- **Тяжёлые deps (нет в prime lock)**: rayon, dashmap, redb, ort (ONNX!), prometheus,
  bincode/lz4_flex/zstd, rusqlite, tree-sitter, petgraph, roaring.
  → `decompiler` тянет rayon (брать без фичи `neural`/`ort`).
- **Много Proposed-статусов** ADR (272, 299-304, 318, 337...) — «первая реализация».
- **Отрицательный результат**: `entropy-ann` (ADR-303) — не тратить время.
- **Placeholder**: `ruvector-core::agenticdb` использует **hash-based placeholder
  embeddings** (не семантические!).
- **rvf/rvAgent** исключены из workspace — брать паттерны, не крейты.

### 26.6 Обновлённый реестр
| Категория | Кол-во |
|---|---|
| L+G+V+M+E+P+S | 11+11+10+5+7+8+20 = 72 |
| R (ruvector, было) | 30 |
| **R3 (ruvector-deep)** | **~25 новых** |
| **ИТОГО** | **~127 фишек** |

### 26.7 Новые оси (ruvector-deep)
- `graph.decompiler` (кластеризация), `graph.modules` (иерархия)
- `graph.cacheWitness` (кэш по content-hash)
- `edit.staleGuard` (staged-workspace)
- `graph.scopes` (физические шарды), `graph.quarantine`
- `graph.receipts` (proof-gate/retrieval-receipt)
- `graph.mmr` (diversity), `graph.triModal` (ANN+BM25+graph)
- `context.masking` (observation masking), `context.voi` (VoI-лестница)
- `graph.qualityEnvelope` (честная деградация)
- `graph.ledger` (TARL), `graph.lineage` (CAMA)


---

## 27. ГЛУБОКОЕ ИССЛЕДОВАНИЕ oh-my-pi (субагент omp-deep)

Отчёт: `.research/oh-my-pi-deep.md` (218 строк). Охват: 12 Rust-крейтов + LSP + agent loop.

### 27.1 Rust-крейты: unsafe-карта (критично для forbid)
| Крейт | LOC | unsafe | Применимость |
|---|---:|---|---|
| **pi-edit** | 30 286 | **0** ✅ | ★★★★★ прямой перенос как `pa-edit` |
| **pi-ast** | 5 293 | **0** ✅ | ★★★★★ для pa-lsp/pa-graph/edit |
| **pi-diff** | 1 020 | **0** ✅ | ★★★★★ замена jsdiff.rs |
| pi-walker | 7 757 | 25 | ★★★★☆ обход (нужна safe-перепись) |
| pi-vfs | 6 408 | 38 | ★★★★☆ URL-пути (safe-перепись) |
| pi-vcs | 13 192 | 1 | ★★★☆☆ git на gix |
| pi-shell | 41 715 | 41 | ★★☆☆☆ (только минимизатор) |
| pi-natives | 86 448 | 405+ | ☆ (N-API, несовместим) |
| pi-builtins | 111 922 | 126 | ☆ |
| pi-iso | 4 845 | 38 | ★★☆☆☆ sandbox PAL |

**Ровно три нужных крейта (pi-edit/pi-ast/pi-diff) — 0 unsafe!**

### 27.2 pi-edit: режимы + token-цена (замер)
| Режим | Механика | Token-цена промпта |
|---|---|---|
| replace | old→new (+replace_all, batch) | ~289 tok |
| apply_patch | Codex `*** Begin Patch` | ~528 tok |
| hashline | `[PATH#TAG]` + PUT/CUT/регистры | ~488 (compact ~422) |
| sloppy | `*** Find`/`*** Replace` | ~555 tok |
| patch | JSON `edits[]` + hunks | ~687 tok |

**hashline guards** (главное):
- **Snapshot tag**: 4-hex = `xxh32` (trailing space/tab/CR обрезаны).
- **seen-lines guard**: anchor не в displayed set → reveal ≤40 строк×512, записать, вернуть ошибку.
- **shift-rescue**: stale-номер спасается только если body несёт текущий контент обоих концов (≥8 символов).
- **no-op limit = 3** (`NOOP_HARD_LIMIT`).
- **block-ops** через tree-sitter (`PUT N*:`), `RESOLUTION_CACHE=512`.
- **Store-лимиты**: 256 путей, 4 версии, 64 MiB, 4 MiB/файл.
- **Fallback**: hashline→replace для моделей kimi/mimo/minimax/deepseek/stepfun/codex-spark/glm-flash-5.3.

### 27.3 LSP-подсистема (9 850 LOC TS)
- **client.ts**: shared mux per-project (Unix socket), `muxServerKey=sha256(cmd,args,cwd,sorted env)`,
  `INIT_FAILURE_BACKOFF=3min`, `pendingDiskWrites` refcount, idle-reaper.
- **tool.ts**: 14 actions, `symbol`+`#N` occurrence-selector, `file:"*"`=workspace.
- **diagnostics.ts**: **inline 500ms / deferred 12s split**; freshness по document-version;
  budgets (single 3s, project 10s, batch 400ms).
- **diagnostics-ledger.ts**: дедуп по identity (message без `path:line:col `).
- **writethrough.ts**: capture minVersions → sync → format → write → notify;
  tsserver `reloadProjects`; batch-id на edit/write.
- **defaults.json**: 57 серверов; auto-detect (root-marker + PATH); lazy=true.

### 27.4 Agent loop + compaction
- **Append-only context**: StablePrefix + AppendOnlyLog → max prefix-cache hit.
- **Speculative execution**: кандидаты tool-calls заранее.
- **Leak mitigation**: harmony `<|call|>` / DSML recovery.
- **Compaction**: pruning (protect 40K tok, cache-aware по lookback), shake (artifact://),
  useless-elision, tool-protection.
- **snapcompact**: PNG-кадры пиксельного шрифта для vision-моделей (необычно!).

### 27.5 НОВЫЕ находки (20)
1. **pi-diff**: jsdiff v9-совместимый Myers, 0 deps, 0 unsafe.
2. **pi-ast::block**: `block_range_at` (outermost named node на строке), `enclosing_block_boundaries` O(window).
3. **bounded parse-cache**: 12 слотов, verify-by-bytes (collision → re-parse).
4. **structural summary**: BFS-unfold до target lines (даёт folded summary → seen-lines).
5. **WASM grammars on demand**: 60+ языков `.wasm.zst`.
6. **ast-grep strictness ladder**: Cst|Smart|Ast|Relaxed|Signature|Template.
7. **apply_edits dedup**: идентичные правки сворачиваются.
8. **pi-walker scan-cache**: TTL=1000ms, 16 entries, 64 MiB.
9. **pi-vfs**: `is_native_local` capability, URL percent-encoding.
10. **CancelToken::heartbeat**: кооперативная отмена CPU-bound.
11. **offline tokenizers**: zstd-словари (o200k/cl100k/Claude/Qwen3/DeepSeek...).
12. **streaming preview**: терпимый JSON-парсер.
13. **`inspect()` без ФС**: статическая проекция payload.
14. **auto-repair**: изоляция culprit-hunk + smol-модель.
15. **inline/deferred diagnostics**.
16. **mux per-project daemon**.
17. **pi-iso**: PAL изоляции (APFS/btrfs/overlayfs/reflink).
18. **pi-vcs**: gix + jj-lib in-process.
19. **bash minimizer**: 25 фильтров.
20. **mnemopi episodic-graph** (SQLite) — идея для pa-graph.

---

## 28. ГЛУБОКОЕ ИССЛЕДОВАНИЕ Serena (субагент serena-deep)

Отчёт: `.research/serena-deep.md` (10 разделов). Охват: 53 tools, symbol.py, diagnostics, LSP-менеджер, contexts/modes, facades, prompts.

### 28.1 Точные числа (уточнено)
- **53 Tool-класса** (29 default + 24 optional; Beta только serena_repl). НЕ 49.
- **77 LanguageServerId** (НЕ 81).
- **65 `@facade_method`** в 8 фасадах.
- **16 contexts + template** (НЕ 26 — часть internal).
- **9 modes + template**.

### 28.2 symbol.py — механика
- **NamePathMatcher**: relative suffix-match / absolute `/`-prefix / `[overload]` / substring (только последний сегмент).
- **SymbolBody ~40 байт**: 4 int + ref на shared lines-буфер (экономия памяти!).
- **EOF-конвенция**: range на `(len(lines),0)` = «до EOF»; иначе `InvalidTextLocationError`.
- **overload_idx** заполняется LS-слоем при дублировании имён среди siblings.
- **Имя НЕ должно содержать `/`** (иначе name_path неразрешим).
- **Retriever**: 25+ методов (find/find_unique/find_by_location/references/implementations/declaration/diagnostics/overview).

### 28.3 Диагностики
- `DiagnosticIdentity` (frozen) + snapshot ДО + diff ПОСЛЕ (только новые).
- `GroupedDiagnostics`: `file→severity→name_path`.
- Привязка к **владеющему символу** (подъём от low-level).
- **`ENABLE_DIAGNOSTICS_DEFAULT=False`** — Serena САМА считает per-edit diag questionable!
- pull-diagnostics + publish fallback.

### 28.4 Contexts/Modes (полные таблицы)
- CLI-контексты (claude-code/codebuddy/zcode/grok) — **жёсткий anti-preference prompt**:
  «Read FORBIDDEN для discovery», таблица маппинга, «Disallowed reasoning».
- `single_project: true` → toolset минимизируется, `activate_project` отключается.
- Порядок наложения: config → context → base_modes → dynamic modes → single-project → project.

### 28.5 Facades + s.info()
- Метод экспонируется ⟺ `@facade_method`.
- `s.info(*items)` — прогрессивное раскрытие + **дедуп типов в рамках сессии**.
- Renderers с прогрессивным сокращением (overview→depth0→counts).

### 28.6 НОВЫЕ находки (24)
1. `symbol_info_budget` (hover time-budget 10s).
2. EOF-конвенция body.
3. SymbolBody ~40 байт.
4. Имя без `/`.
5. overload_idx на LS-слое.
6. Cross-file reference wait (2s).
7. Python-hack variable-refs (не портировать).
8. Import-vs-definition discrimination.
9. `niche`-методы (1 строка + ссылка).
10. `_CAPABILITIES` (dunder→human).
11. **occurrence-id** для replace_in_files: `path:idx@sha256(content:start:end)[:16]` + re-валидация.
12. **ambiguity-детекция** regex (match внутри match → ValueError).
13. backref `$!1` (вместо `\1`).
14. **hooks anti-grep/anti-read** (счётчики 3/3/4, deny с напоминанием, rate-limit 120s).
15. SessionRegistry (LRU+TTL).
16. **TaskExecutor** — сериализация tool-calls (нет race при батчинге).
17. reset-периоды hooks (gap между соседними).
18. trusted_project_path_patterns.
19. line-ending нормализация.
20. `get_apply_docstring_from_cls`.
21. `_wrapped_tool_response`.
22. pull+publish fallback.
23. LanguageServerRegistry (entry-points).
24. auto-registry через `iter_subclasses`.

### 28.7 Топ-10 для переноса
1. name_path-адресация · 2. diagnostics-к-символу · 3. new-vs-existing diff ·
4. SymbolBody+EOF · 5. symbol_info_budget · 6. FS-change notifier · 7. replace_in_files
dry_run+occurrence-id · 8. прогрессивное сокращение · 9. facades+s.info() ·
10. tool-маркеры+auto-registry.

### 28.8 Опасности
1. **Не навязывать per-edit diag** (Serena держит off).
2. Memory-система не нужна (дубль continual harness).
3. JetBrains не нужен.
4. Python-hack не портировать.
5. pull-diagnostics guard (Julia).
6. `_normalize_symbol_name` — убрать parameter-lists, запретить `/`.
7. Параллельные tool-calls — сериализовать (TaskExecutor).
8. occurrence-id re-валидация обязательна.

---

## 29. ОБНОВЛЁННЫЙ СИНТЕЗ (все источники)

### 29.1 Механики: точные контракты
| Источник | Механика | Что даёт prime |
|---|---|---|
| omp pi-edit | EditStore (snapshot/seen-lines/no-op) | provenance для edit-гибрида |
| omp pi-ast | block/summary/parse-cache | AST-слой для pa-graph |
| omp pi-diff | Myers 0-deps | замена jsdiff.rs |
| omp LSP | inline/deferred split, ledger, defaults.json | контракты pa-lsp |
| Serena symbol | name_path, SymbolBody, overload | па-lсп symbols.rs |
| Serena diag | identity+snapshot+diff, grouped | pa-lsp ledger.rs |
| Serena contexts | профили tool surface | изоляция upstream |
| Serena facades | s.info() | прогрессивное раскрытие |

### 29.2 Критический путь (обновлён)
```
pa-diff (pi-diff, 0 unsafe) ─┐
pa-ast (pi-ast, 0 unsafe) ───┼→ pa-graph (SQLite)
pa-edit (pi-edit, 0 unsafe) ─┘
pa-lsp (async-lsp + Serena/omp контракты)
```

### 29.3 Обновлённый реестр
| Категория | Кол-во |
|---|---|
| L (LSP) | 11 |
| G (граф) | 11 |
| V (векторы) | 10 |
| M (память) | 5 |
| E (edit) | 7 |
| P (RAM) | 8 |
| S (Serena) | 20 |
| R (ruvector) | 30 |
| R3 (ruvector-deep) | 25 |
| **O (oh-my-pi-deep)** | **~22** |
| **S2 (serena-deep)** | **~26** |
| **ИТОГО** | **~175 фишек** |

### 29.4 Новые оси (из глубоких разборов)
- `edit.store` (snapshot/seen-lines/no-op limit)
- `edit.fallback` (hashline→replace по модели)
- `edit.occurrenceId` (dry_run + re-валидация)
- `edit.ambiguity` (regex-детекция)
- `lsp.diagSplit` (inline 500ms / deferred 12s)
- `lsp.freshness` (document-version)
- `lsp.mux` (shared per-project daemon)
- `lsp.backoff` (init-failure 3min)
- `context.antiHijack` (жёсткий prompt CLI-клиентов)
- `context.toolsetLayering` (config→context→modes→project)
- `tools.markers` (CanEdit/SymbolicRead/Optional/Beta)
- `tools.autoRegistry` (iter_subclasses)
- `context.progressiveDisclosure` (s.info + дедуп типов)
- `agent.appendOnly` (StablePrefix+AppendOnlyLog)
- `agent.leakRecovery` (harmony/DSML)
- `context.snapcompact` (PNG-кадры, опц.)
- `hooks.antiGrep` (счётчики, deny)
- `session.lru` (SessionRegistry)
