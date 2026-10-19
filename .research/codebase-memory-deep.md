# codebase-memory-mcp (DeusData): глубокий разбор приёмов

Репо: https://github.com/DeusData/codebase-memory-mcp (MIT, 46.4k★, arXiv:2603.27277)
Язык: **чистый C11**, 257 764 LOC в src/, 335 файлов .c/.h. Единый статический
бинарь, ноль зависимостей времени выполнения. 162 языка, 17 MCP-инструментов,
9388 тестовых кейсов.

## Структура и объём (src/, .c)
| Подсистема | LOC | Назначение |
|---|---|---|
| pipeline | 86 844 | индексация: 31 pass-модуль |
| mcp | 41 660 | MCP-сервер, 17 tools, impact-движок |
| cli | 35 475 | CLI |
| daemon | 21 661 | session-координация, watcher |
| foundation | 19 267 | арена, hash_table, compat, limits, lock |
| store | 15 749 | SQLite-схема, digest, impact |
| ui | 12 532 | CBM Lens (3D-граф) |
| discover | 6 784 | обход файлов, .gitignore |
| cypher | 5 939 | Cypher lexer/parser/planner/executor |
| graph_buffer | 2 717 | in-RAM граф до dump в SQLite |
| semantic | 2 633 | 11 сигналов эмбеддингов |
| watcher | 1 861 | авто-переиндексация |
| simhash | 609 | MinHash+LSH |
| traces | 183 | runtime-трейсы |

vendored/: mimalloc, nomic, sqlite3, tre (regex), xxhash, yyjson.

## 31 pass-модуль индексации (pipeline/)
definitions, calls, usages, lsp_cross, semantic, semantic_edges, similarity,
complexity, importance, tests, gitdiff, githistory, docker, k8s, terraform,
infra, ci, operator, configlink, configures, route_nodes, shell, spawns,
pkgmap, compile_commands, cross_repo, diagrams, doclinks, enrichment,
ensemble_routing, parallel.

---

## ПРИЁМ №1: LSP-surface + closure-repair incremental ⭐⭐⭐
**Самое ценное.** `lsp_surface.h`, `pipeline_incremental.c` (4 141 LOC).

### Поверхность файла (SURFACE)
«Всё, что другой файл может потреблять от этого»: defs, зарегистрированные
`pass_lsp_cross`, + registry-only символы. Сериализуется в канонический JSON,
хэшируется (sha256 = `surface_sha`), персистится ПО ФАЙЛУ.

Формат: `{"v":2,"lang":N,"lsp":[...],"reg":[...],"py":...}`
- Порядок полей/массива ФИКСИРОВАН писателем → байтовое равенство = равенство поверхности.
- `v` не совпал → полный ребилд.

### Closure-repair (reverse-dependency)
```
Правка тела файла → surface байт-идентична → пересчитывается ТОЛЬКО сам файл.
Правка поверхности (defs/имена) → тянет ровно файлы с рёбрами в него.
```
Ключевые гарантии:
- Извлечение per-file — **чистая функция контента** → неизменённый зависимый
  не может сам изменить поверхность → **замыкание структурно глубины 1, без fixpoint**.
- **Каждая неопределённость → полный ребилд** (новые файлы, добавленные имена,
  virtual/config, нечитаемые surface-строки, зависимые вне discovery, замыкание
  сверх бюджета). «Declining is never wrong».
- Бюджет: `CLOSURE_BUDGET_PERCENT = 30%`, но `FLOOR_FILES = 8` (малые репо).
- `rehydrate`: десериализация surface НЕперепарсенных файлов обратно в cross-реестры.

**Почему это важно для нас**: переиндексация после одной правки — O(1), не O(repo).

---

## ПРИЁМ №2: Эмбеддинги БЕЗ внешних моделей (11 сигналов) ⭐⭐⭐
`semantic/semantic.h`, 768-dim, edge threshold 0.75 (95% precision на Linux kernel).

### 11 сигналов, объединённых взвешенно
1. **TF-IDF** на метаданных (пересечение словаря)
2. **Random Indexing** с co-occurrence (синонимы внутри кодовой базы)
3. **MinHash** структурный (из "fp"-свойства)
4. **API Signature** (одинаковые callee → связаны)
5. **Type Signature** (одинаковые param/return типы)
6. **Module Proximity** (тот же каталог → boost)
7. **Decorator Pattern** (одинаковые аннотации)
8. **AST Structural Profile** (форма control flow, типы выражений)
9. **Approximate Data Flow** (params→return, params→condition)
10. **Graph Diffusion** (транзитивное замыкание через соседей:
    `combined = (1-α)·combined + α·mean(neighbors)`)
11. **Halstead-Lite** (профиль сложности operator/operand)

### Дешёвые сигналы 8/9/11
Считаются во время AST-walk, хранятся в `properties_json` ("sp" ключ) —
почти нулевая маргинальная стоимость.

### Frequent-token subsampling
Zipfian-токены (int, static, return) доминируют O(occ×window×dim).
Токен с >512 вхождений страйд-сэмплируется; L2-норма сохраняет направление.

### Random Indexing
768-dim, 8 ненулевых на разреженный вектор, окно 5. Не требует обучения.

---

## ПРИЁМ №3: Предобученные токен-векторы (nomic-embed-code) ⭐⭐⭐
`vendored/nomic/code_vectors.{h,bin}` + `code_tokens.h`.

- **40 856 токенов × 768d int8** (unit-normalized, ×127).
- Дистиллированы из **7B-модели** полным инференсом на отфильтрованном словаре.
- «Simulated attention: 3 iterations, K=32, alpha=0.3».
- Источник: huggingface.co/nomic-ai/nomic-embed-code, **Apache 2.0**.
- Формат блоба: `[int32 count][int32 dim]` + count×dim int8.
- Блоб: **31.4 MB** (8 + 40 856×768 байт), вшит через ассемблер `.incbin`
  (`code_vectors_blob.S`). Токены — в `code_tokens.h` (40 864 строки).
  Скрипт извлечения: `scripts/extract_nomic_vectors.py`.
- **int8 vs float32**: float32 НЕ улучшил (co-occurrence memory-bandwidth-bound);
  int8 выигрывает по размеру (30 MB vs 120 MB), равен по runtime.
- **Лицензия Apache 2.0, (c) Nomic AI** — переносимо с атрибуцией.

**Итог**: полноценный семантический поиск кода БЕЗ модели в рантайме —
только таблица токен-векторов + арифметика.

---

## ПРИЁМ №4: RaBitQ-квантизация (4-бит) ⭐⭐
`semantic/rotsq.h` — по статьям Gao et al. (SIGMOD 2024/2025, arXiv:2405.12497).

- Random ±1 diagonal (XXH3-seeded, воспроизводимо) + **Fast Walsh-Hadamard Transform**.
- Вход 768 → pad до 1024 (степень 2). Ортогонально с точностью 1/√D.
- Scalar-квантизация до **4 бит** на координату.
- Оценка IP из кодов: точное SQ-разложение
  `⟨x,y⟩ ≈ D·ox·oy + ox·sy·Σcy + oy·sx·Σcx + sx·sy·Σ(cx·cy)`.
- Память при B=4, D=1024: **512 B кодов + 12 B метаданных** vs 3 072 B для
  768 float32 → **~6× экономия**.
- Σcx/Σcy предвычислены → скоринг пары = один целочисленный dot (u8×u8) + 4 умножения.

---

## ПРИЁМ №5: MinHash + LSH для near-clone ⭐⭐
`simhash/minhash.h` — K=64 сигнатуры из AST-триграмм (xxHash с разными seeds).

- **Leaf-only counting**: leaf-узлы = реальные токены (идентификаторы, литералы),
  языко-агностично (не грамматическая структура). 30 leaf ≈ BigCloneBench 50.
- Порог Jaccard 0.95, max 10 SIMILAR_TO рёбер на узел (против взрыва утилит).
- LSH: 32 bands × 2 rows (threshold ≈ (1/b)^(1/r)).
- K=64 → ±0.12 стандартная ошибка на Jaccard.

---

## ПРИЁМ №6: CALLS с confidence/strategy (честное разрешение)
`pass_calls.c` (69.8K), `lsp_resolve.h` (55.5K, inline-only).

### Стратегии разрешения (по убыванию точности)
`import_map → same_module → unique_name → suffix` (+ LSP-override).

### LSP-override централизован
Раньше каждый pipeline имел свою копию с разными confidence floor
(sequential 0.6, parallel 0.5) → **один проект давал разные рёбра** в зависимости
от режима. Централизовано в `lsp_resolve.h`: `CBM_LSP_CONFIDENCE_FLOOR = 0.6`.

### Properties рёбер
`{callee, confidence, strategy, candidates, args:[{i,e}], line}`.
`args` с индексами и выражениями = **data-flow** бесплатно.

---

## ПРИЁМ №7: In-memory graph buffer → SQLite dump
`graph_buffer.h` — весь граф в RAM во время индексации, потом dump.

- O(1) lookup по qualified_name, dedup рёбер по ключу (`make_edge_key`).
- **3 источника DDL держатся в синхроне**: store.c, graph_buffer dedup, raw-dump writer.
- Temp ID → реальные ID при dump.

---

## ПРИЁМ №8: Схема SQLite (продуманная)
`store/store.c`:

### Таблицы
- `projects(name, indexed_at, root_path)`
- `file_hashes(project, rel_path, sha256, mtime_ns, size)` — инкрементальная классификация
- `nodes(..., UNIQUE(project, qualified_name))`
- `edges(..., url_path_gen GENERATED, local_name_gen GENERATED, UNIQUE(source,target,type,local_name_gen))`
- `nodes_fts` — **contentless FTS5** (`content=''`, unicode61, body=docstring)
- `lsp_surface(project, rel_path, surface_sha, defs_json, ref_bloom, config_ctx)`
- `index_coverage(rel_path, kind, detail)` — честность покрытия
- `observed_calls/observed_paths` — runtime-трейсы (keyed by QN, не node id!)
- `project_summaries` — ADR

### Инженерные решения
- **GENERATED columns** для индексов по JSON (url_path, local_name) без ручного парсинга.
- **local_name_gen**: NULL в UNIQUE не конфликтует → используется `''` (NOT NULL).
  Два именованных импорта из одного specifier = разные рёбра.
- **Contentless FTS5**: хранит только инвертированный индекс, не текст.
  Позволяет кормить camelCase-split на insert, искать по split-токенам.
- **body=docstring**: вопросы словами находят узлы (не только по именам).
- **Schema-compat probe**: `local_name_gen` отсутствует → fail open (не ALTER).
- **Coverage-виды**: `parse_partial` (индексирован, но ERROR-регионы),
  `parse_unusable` (≥80% один регион), skip-фазы (read/extract/oversized).
  «skipped ≠ parse_partial» — важно для читателя.

---

## ПРИЁМ №9: Cypher-движок (свой, 5 939 LOC)
`cypher/cypher.c` — lexer + parser + planner + executor, read-only.
Даёт multi-hop/aggregation/complexity/cross-service. 100k row ceiling.
**Слишком дорого для переноса** — у нас нет графовой БД такого масштаба.

---

## ПРИЁМ №10: Watcher с adaptive polling
`watcher/watcher.h` — git-polling (HEAD/dirty), интервалы 5s + 1s/500 файлов
(cap 60s). Non-git не поллятся без opt-in.
**Ключ**: skipped/failed reindex НЕ коммитит baseline → повторится (не потеряется).

---

## ПРИЁМ №11: Discover (обход файлов)
`discover/` — .gitignore, .cbmignore, symlink handling, inventory фильтры.
Собственная реализация gitignore (не `ignore` crate).

---

## ПРИЁМ №12: Foundation (инфраструктура)
Арены (bump-allocator), hash_table, limits (env-configurable), lock_registry,
compat (потоки/ФС/регекс кроссплатформенно), mem_core (field-owner index).

---

## Что переносить в allspark (по ценности/стоимости)

| # | Приём | Ценность | Стоимость | Вердикт |
|---|---|---|---|---|
| 1 | **LSP-surface + closure-repair** | ★★★ | средняя | ПЕРЕНОСИТЬ (ядро pa-graph) |
| 2 | **11-сигнальные эмбеддинги** | ★★★ | средняя | ПЕРЕНОСИТЬ (частично: 1,3,4,8) |
| 3 | **nomic-embed-code int8** | ★★★ | низкая (данные) | ПЕРЕНОСИТЬ (готовый блоб, Apache 2.0) |
| 4 | **RaBitQ 4-bit** | ★★ | средняя | ПЕРЕНОСИТЬ (экономия памяти) |
| 5 | **MinHash+LSH** | ★★ | низкая | ПЕРЕНОСИТЬ |
| 6 | **CALLS confidence/strategy** | ★★ | низкая | ПЕРЕНОСИТЬ (у нас уже есть!) |
| 7 | **RAM graph buffer** | ★ | низкая | по ситуации |
| 8 | **Схема SQLite** | ★★ | низкая | ПЕРЕНОСИТЬ (копия схемы) |
| 9 | Cypher-движок | ★ | очень высокая | НЕ переносить |
| 10 | Watcher adaptive | ★ | низкая | ПЕРЕНОСИТЬ (notify + git) |
| 11 | Discover | ★ | низкая | есть `ignore`/`walkdir` |
| 12 | Foundation | ★ | низкая | есть Rust-аналоги |

### Синтез с нашим стеком (Rust)
- `rusqlite` + `sqlite-vec` заменяют их C SQLite + int8-vectors.
- `tree-sitter` (pi-ast) заменяет их vendored грамматики.
- `notify` заменяет их polling-watcher.
- `petgraph` даёт graph diffusion / cycles (у них — своё).
- RaBitQ/11-сигналы/nomic-блоб — переносим как алгоритмы/данные.

### Ключевые уроки (что НЕ копировать слепо)
1. **Cypher не нужен** на нашем масштабе.
2. **11 сигналов** — избыточны; берём 4-5 самых дешёвых (TF-IDF, RI, MinHash, AST-profile).
3. **Closure-repair требует «чистой функции per-file»** — у нас LSP-клиент
   (pa-lsp) может это нарушать; нужен детерминированный AST-слой.
4. **int8 > float32** при memory-bound co-occurrence — важно для наших векторов.
5. **Contentless FTS5 + camelCase-split** — точный приём для BM25 по символам.
