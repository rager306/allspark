# АНАЛИЗ ПРОЕКТНОЙ МАТРИЦЫ allspark ПО 7 ОСЯМ

**Вход**: `.research/design-matrix.jsonl` (142 фишки + 55 мета). **Верификация по коду**: `Cargo.toml:49` (`unsafe_code = "forbid"`), `engine.rs:33/446`, `types.rs:498` (trait `AgentTool`), `shared.rs:270-283` (`HostRequestHandlers`), `lifecycle.rs:1022/802`, `settings/types.rs:290` (`Settings` + `extra` flatten), `core.md:3`.
**Доп. контекст**: `design-matrix.md`, `architecture-synthesis-model.md`, `ruvector-full-inventory.md`, `oh-my-pi-deep.md`, `serena-deep.md`, `codebase-memory-deep.md`, `pogocache-techniques.md`, `lsp-synthesis-model.md`, `native-rust-lsp-plan.md`.

**Сводка матрицы**: источники — ruvector 53, oh-my-pi 36, Serena 26, codebase-memory 18, pogocache 9. Приоритеты — P0:12, P1:25, P2:54, P3:51. Механики — native 137, host 3, native+host 1, native+skill 1 (т.е. 96% фишек объявлены как нативный Rust-tool/крейт). unsafe>0 только у 4 фишек (V5=19, V8=15, R11=12, G16=3).

---

## ОСЬ 1. КОНТРАКТЫ

Матрица почти не описывает **точные сигнатуры** — это главный структурный пробел. Ниже — что определено, что отсутствует, и предлагаемые схемы.

### 1.1 Что уже зафиксировано в коде (evidence)

| Контракт | Где | Статус |
|---|---|---|
| trait `AgentTool` (name/description/parameters/label/prepare_arguments/accepts_alias/execute/execution_mode) | `crates/pa-agent/src/types.rs:498` | ✅ есть |
| `SessionEngineConfig.tools: Vec<Arc<dyn AgentTool>>` | `engine.rs:33` | ✅ есть |
| `extra_host_handlers: Option<HostRequestHandlers>` | `engine.rs:42` | ✅ есть (seam) |
| `HostRequestHandlers::register(kind, fn)` | `shared.rs:270-283` | ✅ есть |
| `Settings` с `#[serde(flatten)] extra` (forward-compat) | `settings/types.rs:290,373` | ✅ есть |
| `ToolDefinitionBridge` (pa-core ToolDefinition → AgentTool) | `tool_bridge.rs:17-48` | ✅ есть (шаблон) |
| `lsp` tool JSON-schema (`LspParams::params_schema`) | `crates/pa-lsp/src/types.rs:98` | 🟡 есть, но tool не в workspace |
| `DiagnosticIdentity` (message+range+severity+code+source) | `crates/pa-lsp/src/ledger.rs:20-52` | 🟡 есть |
| `NamePathComponent` + `NAME_PATH_SEP='/'` + overload-idx | `crates/pa-lsp/src/symbols.rs:23-64` | 🟡 есть |
| `apply_workspace_edit` (changes + document_changes/TextEdit) | `crates/pa-lsp/src/client.rs:417-470` | 🟡 есть, file-ops отвергнуты |

### 1.2 ПРОБЕЛ 1-A: нет контракта host_request kinds (критично)

В jsonl упоминается ровно **1** вхождение `host_request` и **1** — `did_change` (в фишке E6). Но цепочка «edit → LSP did_change → diagnostics → graph invalidate» требует минимум 4 kind-ов, ни один из которых не специфицирован как контракт (нет payload-схемы, нет имени, нет ответа):

| kind (предлагается) | direction | payload | reply |
|---|---|---|---|
| `lsp.did_change` | Python(edit) → Rust | `{path, uri, text?, version}` | `{ok, generation}` |
| `lsp.did_open` | session start → Rust | `{path, uri, language_id, text}` | `{ok}` |
| `lsp.did_save` | edit → Rust | `{path, uri}` | `{ok}` |
| `graph.invalidate` | edit/LSP → Rust | `{path, old_hash, new_hash}` | `{scope: "file"|"closure"|"full"}` |
| `graph.warm` | первый запрос → Rust | `{scope?, budget_pct?}` | `{generation, coverage}` |

Фишка L10 (`writethrough`, mech=host) и E6 (`diagnostics_after_edit`, mech=host) реализуют эти вызовы, но их контракт в матрице отсутствует. **Рекомендация**: зафиксировать `FileChangeEvent { path, old_hash, new_hash }` (уже упомянут в `design-matrix.md §3.1`) как единственный общий тип + таблицу kind-ов выше.

### 1.3 ПРОБЕЛ 1-B: DB-схема SQLite не выписана как контракт

G1 (`sqlite_graph`) описан словами («nodes + edges + GENERATED-колонки»), но нет DDL. Из `codebase-memory-deep.md` (приём №8) известны таблицы, но матрица их не фиксирует. Нужен точный контракт, иначе G2/G3/G5/G7/G11/L11 не стыкуются. Предлагаемая схема:

```sql
projects(name TEXT PRIMARY KEY, root_path TEXT, indexed_at INTEGER, schema_version INTEGER);
file_hashes(project TEXT, rel_path TEXT, sha256 TEXT, mtime_ns INTEGER, size INTEGER,
            PRIMARY KEY(project, rel_path));
nodes(id INTEGER PRIMARY KEY, project TEXT, label TEXT, name TEXT, qualified_name TEXT,
      file TEXT, start_line INT, end_line INT, props TEXT, generation INTEGER,
      UNIQUE(project, qualified_name));
edges(id INTEGER PRIMARY KEY, project TEXT, source INT, target INT, type TEXT, props TEXT,
      confidence REAL, strategy TEXT,
      local_name_gen TEXT GENERATED ALWAYS AS (json_extract(props,'$.local_name')) VIRTUAL,
      UNIQUE(source,target,type,local_name_gen));
nodes_fts VIRTUAL TABLE USING fts5(name, qualified_name, body, content='', tokenize='unicode61');
lsp_surface(project TEXT, rel_path TEXT, surface_sha TEXT, defs_json TEXT,
            ref_bloom BLOB, config_ctx TEXT, surface_v INTEGER, PRIMARY KEY(project,rel_path));
index_coverage(project TEXT, rel_path TEXT, kind TEXT, detail TEXT);
query_cache(key_hash TEXT PRIMARY KEY, query_vec BLOB, result_json TEXT, generation INTEGER,
            created_at INTEGER, hits INTEGER);
graph_generation(project TEXT PRIMARY KEY, generation INTEGER, built_at INTEGER, coverage_json TEXT);
```

**Находка**: `graph.schemaVersion` объявлен в `design-matrix.md §4.4`, но в jsonl **0** вхождений `schemaVersion` и `surface.v`. Оси схемы/формата в матрице не заведены → миграция не специфицирована.

### 1.4 ПРОБЕЛ 1-C: surface-JSON формат

G7/L11 опираются на «surface-хэш», но формат `defs_json` и правило канонизации не зафиксированы. codebase-memory использует `{"v":2,"lang":N,"lsp":[...],"reg":[...],"py":...}` с **фиксированным порядком полей** (`codebase-memory-deep.md`, приём №1). **Критично**: в prime `serde_json` собран с `preserve_order` (`Cargo.toml:28-33`) → порядок ключей детерминирован только если писать вручную; сортировка через `BTreeMap` нарушит байтовое равенство. Нужен явный контракт: «surface = JSON с ключами в лексикографическом порядке, массивы отсортированы по (kind, name, start_line)». Без этого closure-repair будет ложно инвалидировать.

### 1.5 ПРОБЕЛ 1-D: конфликт `DiagnosticIdentity` между источниками

Два определения в матрице:
- L8 (`oh-my-pi`): «дедуп по identity (message **без** `path:line:col` префикса)».
- L9/E6 (`Serena`): `DiagnosticIdentity` = message + range + severity + code + source.

В `crates/pa-lsp/src/ledger.rs:20-52` реализован **Serena-вариант** (range ВХОДИТ в identity; комментарий: «diagnostic that merely moved counts as new»). Это **прямо противоречит** L8 (oh-my-pi: range исключён). Разные семантики дадут разное поведение «только новые». **Рекомендация**: оставить Serena-вариант (он в коде) и переформулировать L8 как «identity включает range»; либо ввести два режима (`ledger.strict` / `ledger.loose`).

### 1.6 ПРОБЕЛ 1-E: `WorkspaceEdit` file-ops

L5 (`rename_symbol`) требует WorkspaceEdit, L18 (`rename_file`) — willRenameFiles/didRenameFiles. В `client.rs:417-470` `apply_workspace_edit` **отвергает** file create/rename/delete («file-operation workspace edits are not supported yet»). L18 помечен P3 и зависит от L5+L2, но контракт для file-ops (ResourceOperation) не определён. **Находка**: L18 и L19 (code_actions) несовместимы с текущим `apply_workspace_edit`.

### 1.7 ПРОБЕЛ 1-F: `name_path` — коллизия с ограничением «имя без `/`»

L3 notes: «Имя символа НЕ должно содержать `/`». В `symbols.rs` `NAME_PATH_SEP='/'` и парсинг `name[idx]`. Но матрица не специфицирует: (а) как эскейпить `/` в имени (Rust: операторы `impl Add/Sub`, трейты `Foo/Bar` — валидные имена!); (б) что делать с generics (`Vec<T>`) и lifetime. **Находка**: Rust-имена с `/` реальны (path-типы в трейт-импл), правило Serena «не должно содержать `/`» для Rust **недостаточно** — нужен escape или absolute-prefix-форма.

### 1.8 ПРОБЕЛ 1-G: settings-ключи не имеют контракта типов

`design-matrix.md §2` перечисляет ~40 осей, но `settings/types.rs` знает `KNOWN_FIELDS` (load.rs:10-50) — новый ключ `lsp`/`graph`/`edit` нужно **добавить в KNOWN_FIELDS**, иначе bad-typed значение молча уйдёт в `extra` (load.rs:84). Ни один из ключей `lsp.enabled`, `graph.enabled`, `edit.mode` в матрице не имеет объявленной структуры (bool vs object). **Рекомендация**: `pub lsp: Option<LspSettings>`, `pub graph: Option<GraphSettings>`, `pub edit: Option<EditSettings>` по образцу `CompactionSettings` (`types.rs:94`), +3 записи в KNOWN_FIELDS.

### 1.9 ПРОБЕЛ 1-H: нет контракта surface-JSON для `generation`

`generation` встречается 2× в jsonl, но `client.rs:35-36` уже ведёт `generation: HashMap<uri,u64>` для диагностик. При этом `graph.generation` (в мета-осях observability) — это версия **индекса**, а не документ-версия LSP. **Коллизия имён**: две разные сущности называются `generation`. См. Ось 4.

---

## ОСЬ 2. КЛЕЙ (glue)

### 2.1 Карта зависимостей: хабы и изолированные

Построен неориентированный граф `intersects[]` (142 узла, все ссылки валидны, кроме **1 dangling**: `G10 -> M-harness`, фиктивный ID).

**Хабы (degree):**

| Фишка | degree | роль |
|---|---|---|
| **G3** graph_search | 26 | супер-хаб: 22 входящих (векторы, MMR, фильтры, recall-ANN) |
| **E1** edit_hybrid | 17 | вход edit-слоя (11 входящих) |
| **G1** sqlite_graph | 14 | хранилище-центр |
| **G7** closure_repair | 14 | инкрементальный контур |
| **L6** refs_def_hover | 13 | LSP-запросы |
| **G2** indexing | 13 | индексатор |
| **L4** diagnostics | 12 | диагностики |
| **E6 / O2 / V3** | 10 | diag-after-edit / AST-block / эмбеддинги |

**Изолированные (degree 0):** P8 (cross_compile), O8 (leak_recovery), O10 (bash_minimizer), R14 (prometheus_metrics).

### 2.2 ПРОБЕЛ 2-A: 63 цикла в графе зависимостей (критично)

DFS-детект циклов нашёл **63 цикла**. Это не обязательно дефект (граф смешивает «зависит от» и «связано с»), но матрица не различает **тип** ребра, поэтому порядок реализации невыводим. Примеры:

```
E1 -> E2 -> E1                       (edit ↔ ast_edits)
G2 -> G1 -> G2                       (индексатор ↔ хранилище)
G3 -> V1 -> G12 -> V2 -> G3          (поиск ↔ FTS5 ↔ BM25+RRF)
M1 -> M2 -> M1 / M1 -> M3 -> M1      (compaction ↔ decay/eviction)
L1 -> L2 -> L1                       (client ↔ tool)
L4 -> L8 -> L4                       (diagnostics ↔ ledger)
Гигантский цикл из 20 узлов: E1→E2→O2→G2→G1→G3→V1→G12→V2→G13→V3→V6→G15→R10→G7→L11→L1→L2→L4→L8→E6→E1
```

**Рекомендация**: ввести поле `intersects_kind: "depends_on" | "shares_api" | "feeds"` (сейчас `intersects[]` — плоский список). Иначе фазовый план (`design-matrix.md §6`) недоказуем.

### 2.3 ПРОБЕЛ 2-B: 20 нарушений фазового порядка

Сопоставив `intersects[]` с назначенными фазами, найдено **20 рёбер**, где фишка зависит от фишки из **более поздней** фазы. Худшие:

| Зависимость | фазы | Проблема |
|---|---|---|
| L4(1) → L9(7) | diagnostics → diag-to-symbol | L4 «зависит» от того, что даёт L9, но L9 позже |
| L6(1) → G3(6), L6(1) → G4(6) | refs/def → graph_search/trace | LSP-слой ф.1 ссылается на граф ф.6 |
| G2(2) → G1(3) | indexer → sqlite_graph | индексатор без хранилища |
| G1(3) → G7(5), G1(3) → L11(5) | graph → closure/surface | — |
| O2(2) → E2(8) | AST-block → ast_edits | ф.2 зависит от ф.8 |
| O1(2) → E1(8) | pi-diff → edit | — |
| L10(7) → E1(8) | writethrough → edit | L10 не может тестироваться без E1 |

Также **96 из 142 фишек не имеют фазы** вообще (нет в `items` ни одной phase-записи): E7-E11, G6/G8/G9/G10/G13/G15/G17/G18, L3/L5/L13-L20, M4-M7/M9/M11, O4-O12, P2/P3/P5-P8, R2/R3/R5/R6/R10-R22, S1-S20, V1/V3/V4/V6/V8/V10-V20. Фазовый план покрывает ~32% фишек.

### 2.4 Цепочка edit→LSP→diagnostics→graph→reindex: разрывы и race

Заявленная цепочка (`design-matrix.md §3.1`):
```
edit (запись файла)
  → host_request "lsp.did_change"  → pa-lsp обновляет документ
  → diagnostics-ledger             → новые диагностики
  → pa-graph: инвалидация surface  → closure-repair
  → graph_search переиндексирует изменённый файл
```

**Разрыв 2-C (race): порядок LSP vs graph не определён.** Кто первый — `did_change` (LSP) или `graph.invalidate`? Если инвалидация идёт по `new_hash` файла, а LSP ещё не получил текст → diagnostics устареют. В `client.rs` есть `generation` per-URI и `apply_workspace_edit`, но **нет** барьера «файл записан → LSP увидел → граф увидел». **Нужен контракт**: `did_change` синхронный и возвращает `generation`; `graph.invalidate` принимает `(path, old_hash, new_hash, lsp_generation)`.

**Разрыв 2-D (кто пишет файл?).** Фишка L5 `rename_symbol` применяет WorkspaceEdit напрямую (`apply_workspace_edit` → `std::fs::write`, `client.rs:445`), минуя edit-tool и его guards (E3 stale-tag, E4 seen-lines, R4 staged-workspace). **Это двойной путь записи в файл** → guards обходятся при rename. **Рекомендация**: rename тоже должен идти через `EditWriter`/staged-контур, либо явно задокументировать «LSP-правки не проходят guards».

**Разрыв 2-E (цикл edit→diag→edit).** E6 (`diagnostics_after_edit`) в default off (Serena: `ENABLE_DIAGNOSTICS_DEFAULT=False`). Но если включить + L10 writethrough (default `true` в `design-matrix.md §2.1`) → после каждой правки запускается format+did_change+diag. При авто-правках (E10 auto_repair) возможен цикл «правка → diag → auto-repair → правка». **Нет счётчика/предохранителя** против этого.

**Разрыв 2-F (адаптер неявен).** `graph_search` (G3, native tool) и `find_symbol` (S10, native tool) оба «ищут символ», но первый — по BM25 графа, второй — по LSP. Их `intersects` (G3↔S10 через `in`) не описывают **приоритет**: кто из них вызывается моделью? В матрице нет решения «graph-first vs LSP-first». Это ключевой адаптер-пробел.

### 2.5 ПРОБЕЛ 2-G: `M-harness` — фиктивная ссылка

G10 (`adr_memory`) пересекается с `M-harness`, которого нет в реестре. Либо это continual harness (внешняя система), либо опечатка. **Находка**: заменить на реальный ID или пометить `external:continual-harness`.

---

## ОСЬ 3. ЛОГИКА

### 3.1 Индексация: полная / инкрементальная (closure-repair)

Логика closure-repair (G7) в матрице описана одной строкой + note «Каждая неопределённость → полный ребилд». Из `codebase-memory-deep.md` (приём №1) известно точнее:
```
правка тела файла → surface байт-идентична → пересчёт ТОЛЬКО файла
правка surface   → замыкание зависимых (глубина 1, без fixpoint)
неопределённость → полный ребилд (CLOSURE_BUDGET_PERCENT=30%, FLOOR_FILES=8)
```

**Дыра 3-A (недоспецифицировано состояние `rehydrate`).** closure-repair требует десериализовать surface **неперепарсенных** файлов обратно в реестры (`rehydrate`). В матрице нет фишки для rehydrate — G7 её подразумевает, но не выделяет. При отсутствии — либо полный ребилд (потеря смысла инкрементальности), либо неполный граф.

**Дыра 3-B (кто триггерит repair).** Есть G5 (git-diff→символы), G11 (file_hashes), S16 (fs_change_notifier), R8/R9 (delta). Но **не определено**, какой из них — первичный триггер: git-polling (G5), mtime-polling (S16) или delta-stream (R8/R9)? Это три разных потока управления. codebase-memory использует adaptive git-polling; Serena — mtime-polling. **Рекомендация**: выбрать один (предлагается `notify`-watcher, S16) и сделать остальные производными.

**Дыра 3-C (LSP ломает «чистую функцию per-file»).** Ключевая гарантия closure-repair — «извлечение per-file есть чистая функция контента». Но если `lsp_surface` (L11) питается от LSP-сервера, а не от tree-sitter, то результат зависит от состояния сервера (не детерминирован). `architecture-synthesis-model.md §15.10` уже отметил: «базировать на pa-ast (детерминированный), LSP — как override». В матрице это **не зафиксировано** — G7∩L11 выглядит как «равные партнёры». **Критично для корректности.**

**Дыра 3-D (surface budget сверх бюджета).** Матрица не описывает, что происходит, когда замыкание превышает `graph.indexBudgetPct=30`. `codebase-memory` делает полный ребилд; но в фоне (async) это может занять минуты. Нет состояния «rebuild in progress» и нет политики «что видит поиск во время ребилда» (старый индекс? пустой? partial?).

### 3.2 Поиск: BM25 + RRF + graph

Заявлено (G3, V2, G13): BM25 + dense + RRF(k=60) + graph-traversal, over-fetch k*4. Логические пробелы:

**Дыра 3-E (нормализация скоров).** RRF работает на рангах (устойчив), но `ScoreFusion` (V2) — на сырых скорах, где BM25 неограничен, а cosine ∈ [-1,1]. В матрице **не сказано, какой fusion используется по умолчанию** (`graph.hybrid` enum bm25/vector/hybrid — но не RRF vs RSF vs ScoreFusion). Без выбора — недетерминированный ранкинг.

**Дыра 3-F (query-cache инвалидация).** M10 (`query_cache_semantic`): «новый запрос близок (cosine) к отвеченному → кэш». Но при изменении графа кэш надо инвалидировать. M11 (`witness_keyed_cache`) даёт ключ=content-hash. **Не описано**, как M10 (semantic, по вектору запроса) сочетается с M11 (witness, по состоянию репо): если запрос похож, но граф изменился → вернётся устаревший результат. Нужен контракт: `query_cache` ключ = `(query_vec_cluster, graph_generation)`.

**Дыра 3-G (что ищет `graph_search` по умолчанию).** G3 описывает «BM25 + regex name/qn + semantic_query». Но каков дефолт при пустом `semantic_query` (векторов нет / `graph.semantic=false`)? Fallback на чистый BM25 не описан.

### 3.3 Правка: preview → stage → write → diagnostics

E1-E11 описывают edit, но поток не сведён в один конвейер. По `oh-my-pi-deep.md §2`: `ArgStream → ArgSnapshot → preview(streaming) → stage → StagedFile[] → EditWriter`. В матрице:

| Стадия | Фишка | Пробел |
|---|---|---|
| preview | E8 (streaming_preview) | P3, «терпимый JSON-парсер» — не связан с E1 |
| stage | R4 (staged_workspace) | P1, но **не в фазе 8 с E1** (R4 в фазе 8, ок) |
| guards | E3/E4/E5 | есть |
| write | E1 | есть |
| diagnostics | E6 | host, default off |
| repair | E10 | P3, требует smol-модель (нет в списке моделей) |

**Дыра 3-H (порядок guards).** E3 (stale-tag), E4 (seen-lines), E5 (noop-limit), R4 (staged-workspace hash+revision) — четыре guard-а, но **не определён порядок** их применения. Если R4 (hard reject) идёт первым, E3/E4 (reveal+retry) никогда не сработают. Нужна таблица приоритета: R4 (binding) → E3 (stale) → E4 (seen) → E5 (noop).

**Дыра 3-I (E4 требует provenance, которого нет).** E4 note: «Требует provenance из read/search». Но read — это Python `open()` (не tool), а search — `graph_search`/rg. **Не определено**, как edit узнаёт «displayed set» (какие строки модель видела). В oh-my-pi это EditStore внутри одного процесса; в prime read идёт через REPL/fs — провенанс надо явно собирать. **Крупный пробел.**

**Дыра 3-J (E11 fallback по модели).** E11: «Fallback hashline→replace для kimi/mimo/minimax/...». Список моделей захардкожен. В prime модели задаются через провайдеров/`models.json` — маппинг «модель → режим edit» не определён (нет оси).

### 3.4 Недоспецифицированные состояния

- **LSP не установлен** (риск high): матрица говорит «graceful degrade + fallback rg», но fallback rg **не покрывает** semantic actions (definition/references/rename) — только текст. Что вернёт `lsp(action="references")`? Ошибку или пусто? Q9 открыт.
- **Индекс пуст/строится**: `graph_search` при `graph.enabled=true`, но индексации нет. Не описано.
- **DB повреждён**: риск «.db повреждён» → «WAL + snapshot», но нет фишки восстановления (rebuild из file_hashes?).
- **Concurrent субагенты**: Q7 открыт (один .db на проект/сессию). Не решено → race на WAL не исключён.

---

## ОСЬ 4. СЕМАНТИКА

Матрица собрана из 5 источников с **разной терминологией**. Ниже — разбор каждой сущности и найденные коллизии.

### 4.1 Терминологические коллизии (критично)

| Термин | Значение A | Значение B | Риск |
|---|---|---|---|
| **surface** | L11/G7: LSP-поверхность файла (defs+reg, content-hash) — codebase-memory | `design-matrix.md` заголовки «поверхность» в общем смысле; `serde_json` «surface» = любой JSON-контракт (`Cargo.toml:28`) | смешение «surface-хэш» и «JSON-surface» в одном документе |
| **generation** | LSP: `client.rs:35` version документа (bump на publishDiagnostics) | meta observability `graph.generation` = версия индекса | **2 разные сущности**; в chain «edit→diag» и «graph» обе нужны одновременно |
| **witness** | M6/R17: крипто-цепочка (HashChain/Merkle MMR) записи | G16/decompiler: WitnessTree (Jin-Sun-Thorup) — сертификация min-cut | разные механизмы, один термин |
| **coherence** | M1: `I=α·rec+β·freq+γ·coh` (cosine к контексту) | M2: `graph_gate` (in-degree голосование); ruvector-coherence-hnsw (coherence-gated beam) | 3 разных смысла «когерентности» |
| **repair** | G7: closure-repair (переиндексация) | M7/R10: HNSW repair (после удалений) | «repair» без квалификатора неоднозначен |
| **condense** | G15: graph condensation (сжатие) | `ruvector-full-inventory.md N15`: «condensation» ≠ SCC-конденсация | явно предупреждено, но в матрице G15 не помечено |
| **confidence** | G8: CALLS confidence (0..1, стратегия резолва) | CAMA (M5): «downgrade-only confidence» (памяти) | две шкалы уверенности |
| **capability** | R2: 64-бит CapMask (права внутри ANN) | meta `mechanic` values; `graph.capabilities` | — |
| **anchor** | R4: TransitionAnchor (запись до мутации) | E4: seen-lines anchor (строка-якорь) | одно слово, два контракта |
| **revision** | R4: revision_id (staged-workspace) | git revision (G5) | — |
| **delta** | R8: Delta/DeltaStream (event sourcing) | `graph.deltaIndex` (R10: HNSW-инкремент) | разные уровни (граф vs ANN) |

### 4.2 Сущности, у которых НЕТ определения в матрице

| Сущность | Упоминается в | Определение есть? |
|---|---|---|
| `name_path` | L3, S10, G4 | 🟡 частично (Serena-формы), нет escape-правил для Rust |
| `surface` (формат) | G7, L11 | ❌ только слово |
| `generation` (индекс) | meta | ❌ нет типа/семантики |
| `confidence` (шкала) | G8 | 🟡 «0..1», но нет границ применения |
| `coherence` (метрика) | M1, M2 | ❌ нет формулы в матрице (есть в ruvector-inventory) |
| `witness` (контракт) | M6, R17 | ❌ нет структуры цепочки |
| `lineage` | M5 | 🟡 «causal_parent», нет формата |
| `quality-envelope` | M9 | 🟡 перечислены enum-ы, нет wire-формы |
| `capability` (маска) | R2 | ❌ нет битовой раскладки |

### 4.3 Конфликты смыслов между источниками

**4.3-A. «symbol» — три источника, три адресации.**
- oh-my-pi (L2): `symbol` + `#N` occurrence-selector (позиция + номер).
- Serena (L3/S10): `name_path` (без позиции, suffix/absolute).
- codebase-memory (G4): `qualified_name` (qn, стабильный ключ в БД).

В `pa-lsp/src/types.rs:73` реализован **Serena-путь** (`symbol` как name_path). Но G3/G4 (граф) работают с `qn` из SQLite. **Не определено**, совпадают ли `name_path` (Serena) и `qualified_name` (граф) — а это ключ к связке LSP↔graph (L6∩G3/G4). Если форматы разные → нужен адаптер name_path↔qn. **Крупная семантическая дыра.**

**4.3-B. «diagnostics identity» — два определения (см. Ось 1.5).** L8 (без range) vs L9/E6 (с range). Код выбрал Serena-вариант.

**4.3-C. «evidence/receipt» — R14 vs M6 vs R17.**
- M6 (`witness_receipts`): proof-gate на запись + retrieval-receipt на чтение.
- R17 (`proof_gate`): HashChain/Merkle MMR gate + WriteReceipt.
- R14 (`prometheus_metrics`): метрики.

M6 и R17 — **дубликаты** (оба про HashChain/Merkle MMR write-gate). В матрице они разнесены (memory vs ruvector), но описывают один механизм. **Рекомендация**: объединить.

**4.3-D. «quality» — G9 (coverage_honesty) vs M9 (quality_envelope).**
G9: `index_coverage` (parse_partial/parse_unusable/skip) — честность **индекса**.
M9: `RetrievalQuality` (Full/Partial/LayerAOnly/Degenerate/BruteForce) — качество **ответа**.
Разные уровни, но оба называют «coverage/quality». Их стоит связать: degraded retrieval ← low coverage. В матрице связи нет.

**4.3-E. «budget» — три бюджета.**
- L16: `symbol_info_budget` (time-budget hover 10s).
- M8: `bounded_rag` (token-budget).
- G7: `indexBudgetPct` (30% индексации).
- L12: inline 500ms/deferred 12s.
Четыре «бюджета» с разными единицами — нет общего типа `Budget`.

---

## ОСЬ 5. ОПТИМИЗАЦИИ

### 5.1 Инвентарь оптимизаций по классам

| Класс | Фишки | Ценность | Стоимость |
|---|---|---|---|
| Кэши | L11 (surface), M10 (query semantic), M11 (witness-keyed), O3 (parse), R5 (schema ReCache), G12/V1 (FTS5) | высокая | низкая-средняя |
| Инкрементальность | G7 (closure), R8/R9 (delta), R10 (delta-index), R11 (drifting), R12 (condenser), G15 (condense) | высокая | средняя-высокая |
| Eviction | M3 (2-random), M1 (coherence), M2 (decay) | средняя | низкая |
| Компрессия | V4 (int8), V5 (RaBitQ), V7 (turbovec), V8 (Turbo4), V9 (nomic) | средняя | средняя |
| Ленивость | L1 (lazy spawn), L12 (deferred diag), L14 (mux), R12 (lazy condense) | высокая | низкая |
| ANN-оптимизации | V10-V20 (ACORN, matryoshka, speculative, recall-targeted/bounded, filters, LSM, PDX) | средняя | средняя |
| Layout/CPU | P1-P8 (shards, robin-hood, 6-byte ptr, single-alloc) | низкая | высокая |

### 5.2 Максимум при минимуме сложности (рекомендация — брать первым)

| Фишка | Почему |
|---|---|
| **G7 closure-repair** | Превращает переиндексацию O(repo)→O(1) на правку. Ядро всей ценности графа. Уже в P1. |
| **L12 inline/deferred split** | 500ms/12s — edit не блокируется на медленном сервере. Дёшево (low), огромный UX-выигрыш. |
| **L15 init_failure_backoff** | Negative-cache 3min — сломанный сервер не ре-спавнится. Дёшево. |
| **M3 2-random eviction** | O(1) без LRU-структур. Дёшево, снимает риск OOM. |
| **M10 query-cache semantic** | Прямой выигрыш для graph_search (агенты повторяют запросы). P1, low. |
| **V2 BM25+RRF** | Готовый safe-крейт (`ruvector-hybrid`, `#![forbid]`). P1, low. |
| **M1 coherence_compaction** | Готовый safe-крейт (`ruvector-agent-memory`). P2, low. |
| **L11 surface-cache** | Инвалидация по хэшу — фундамент closure-repair. |
| **O3 bounded parse-cache** | 12 слотов, 4MiB, verify-by-bytes. Дёшево, ускоряет AST. |

### 5.3 Преждевременно (P3, отложить)

| Фишка | Почему преждевременно |
|---|---|
| **P1-P8** (pogocache layout: shards, robin-hood, 6-byte ptr, single-alloc) | Микрооптимизации RAM-графа. `architecture-synthesis-model.md §15.7` уже решил: «начать с dashmap+petgraph». 12 фишек P3/high-cost. **Отложить до подтверждённой нехватки памяти.** |
| **V5/V8 RaBitQ/Turbo4** | Требуют safe-переписи (unsafe=19/15). Есть int8 (V4) — проще. |
| **V12 MaxSim, V13 GNN-rerank** | Multi-vector/нейрореранк — избыточно на масштабе репо-агента. |
| **G14 Cypher, G17 mincut, R19 cognitive-container, R22 persistent-homology** | Тяжёлые (25K-55K LOC), нет сценария. `architecture-synthesis-model.md §9` уже отверг Cypher. |
| **O11 snapcompact** | Требует vision-модель (не в списке). Экзотика. |
| **R20 mcp-brain** | Пересекается с continual harness. Опционально. |
| **V9 nomic-блоб (+31MB)** | `architecture-synthesis-model.md §15.8`: опционально, +31MB бинаря. |
| **P3 V16/V17 recall-targeted/bounded** | Преждевременно без базового ANN. |

### 5.4 Наблюдение: 51 фишка P3 (36% матрицы)

Из 142 фишек **51 — P3** и **12 из них high-cost** (L14 mux, G14 Cypher, G17 mincut, E10 auto-repair, P1/P3 RAM-layout, O5 wasm-grammars, O9 speculative-exec, O11 snapcompact, R19 container, R20 brain, R22 homology). Это признак **переобогащения матрицы**: реальная реализуемая ценность ~30-40 фишек. **Рекомендация**: вынести P3 в отдельный «backlog» и не считать частью плана.

### 5.5 Компрессия — что выбрать

Матрица предлагает 5 схем (int8 V4, RaBitQ V5, turbovec V7, Turbo4 V8, nomic V9). Из `ruvector-full-inventory.md §5.1`: RaBitQ=19 unsafe, turboquant=15 unsafe, turbovec=**0 unsafe**. **Рекомендация**: int8 (V4) → turbovec (V7, safe multi-bit) при нужде → RaBitQ/Turbo4 только если безопасно перепишут. nomic (V9) — отдельно (данные, не алгоритм).

---

## ОСЬ 6. МЕТРИКИ

Матрица объявляет метрики в 2 местах: meta-запись `observability` (axes: graph_index_ms, graph_query_ms, lsp_call_ms, edit_guard_hits, graph.cache_hit_rate, graph.trace, graph.stats, graph.generation, lsp.trace) и `design-matrix.md §4.2` (6 метрик). Но **нет** метрик качества графа/LSP/поиска — только latency/cache. Это крупный пробел.

### 6.1 Метрики по подсистемам (предлагаемый набор)

| Подсистема | Метрика | Тип | Источник-фишка |
|---|---|---|---|
| **Индексация** | `graph_index_ms` (полная/инкрементальная) | histogram | G2, G7 |
| | `graph_files_indexed` / `graph_files_skipped` | counter | G2, G9 |
| | `graph_index_coverage_pct` | gauge | G9 |
| | `graph_closure_size` (сколько файлов в замыкании) | histogram | G7 |
| | `graph_full_rebuild_count` (срабатываний «неопределённость→ребилд») | counter | G7 |
| | `graph_surface_rehydrate_count` | counter | G7 (дыра 3-A) |
| **Поиск** | `graph_query_ms` (p50/p95/p99) | histogram | G3, V2 |
| | `graph_cache_hit_rate` (query + surface + witness) | gauge | M10, M11, L11 |
| | `graph_recall_at_k` (по golden-набору) | gauge | V16, V17 |
| | `graph_fusion_strategy` (rrf/rsf/score) | label | V2 |
| **LSP** | `lsp_call_ms` (по action) | histogram | L2 |
| | `lsp_servers_live` / `lsp_servers_failed` | gauge | L7, L15 |
| | `lsp_init_failures` / `lsp_backoff_hits` | counter | L15 |
| | `lsp_diagnostics_fresh_ratio` (новые/все) | gauge | L8, L13 |
| | `lsp_pull_vs_publish_ratio` | counter | L4 |
| **Edit** | `edit_guard_hits` (по guard: stale/seen/noop/binding) | counter | E3, E4, E5, R4 |
| | `edit_noop_streak` | gauge | E5 |
| | `edit_apply_ms` (preview/stage/write) | histogram | E1 |
| | `edit_diagnostics_new` (после правки) | histogram | E6 |
| **Память** | `graph_evictions_total` (по политике) | counter | M1, M2, M3 |
| | `graph_compaction_ms` | histogram | M1 |
| | `graph_memory_mb` | gauge | meta `graph.maxMemoryMb` |

### 6.2 Как измерять качество (не только latency)

**Качество графа** — нет метрики в матрице. Предлагается:
- `graph_edge_precision/recall` на golden-наборе (CALLS vs LSP-override). codebase-memory использует LSP-override с floor 0.6 — можно сравнивать tree-sitter-рёбра с LSP-рёбрами → precision/recall.
- `graph_cycle_count` (petgraph) — регрессия графа.
- `graph_orphan_node_ratio` (узлы без рёбер).
- `graph_coverage_by_language` (G9: parse_partial/unusable/skipped).

**Качество LSP** — предлагается:
- `lsp_resolve_accuracy` (definition/references совпали с ожидаемым в golden).
- `lsp_diagnostics_identity_stability` (L8 vs L9: сколько «новых» при неизменном файле — должно быть 0).

**Качество поиска** — предлагается:
- `search_recall_at_k`, `search_mrr` (mean reciprocal rank) на golden-запросах.
- `search_duplicate_ratio` (до/после MMR V14).

### 6.3 Детект деградации

| Сигнал | Порог | Реакция |
|---|---|---|
| `graph_query_ms` p95 растёт | > 2× baseline | re-check индекса/кэша |
| `graph_cache_hit_rate` падает | < 0.3 | инвалидация слишком агрессивна (M11 generation drift) |
| `graph_full_rebuild_count` растёт | > N/час | closure-repair деградировал (дыра 3-A/3-C) |
| `lsp_servers_failed` растёт | > 0 | L15 backoff сработал, нужен fallback (Q9) |
| `graph_recall_at_k` падает | < 0.9 | ANN-параметры деградировали (R11 drifting trigger) |
| `edit_guard_hits` спайк | > X/turn | модель бьётся о guards → плохой prompt |

### 6.4 Что в телеметрию

В prime уже есть `pa-telemetry` (leaf, fan-in=1792). `R14 (prometheus_metrics)` — изолированная фишка (degree 0). **Рекомендация**: не тащить Prometheus (тяжёлая dep), а эмитить метрики через существующий `pa-telemetry`. Метрики выше — в структурном виде (counter/gauge/histogram). `graph.trace`/`lsp.trace` — debug-логи, default off (как объявлено).

**Находка 6-A**: meta-ось `observability` содержит `graph_index_ms`, `graph_query_ms`, `lsp_call_ms`, `edit_guard_hits`, `graph.cache_hit_rate` — но эти же имена **не совпадают** с `design-matrix.md §11.4` (`graph_index_ms` vs `graph_index_ms` — ок, но `lsp_call_ms` vs `lsp_call_ms`). Единообразно. Однако `graph.generation` в observability и `graph.generation` в isolation/functional — **дублирование класса** (см. Ось 1.9).

---

## ОСЬ 7. ТЕСТЫ

В prime **уже есть** сильная тестовая инфраструктура (проверено по коду):
- `crates/pa-core/tests/golden_replay.rs` — golden-replay с байт-сравнением с TS (`golden_edit_group_matches_ts`, `golden_bash_group_matches_ts`, `golden_schema_group_matches_ts`, `golden_ipython_group_matches_ts`). Корпус в `tests/golden/corpus/*.json` (bash/edit/ipython/preview/schema/truncate).
- `crates/pa-core/tests/prompt_guards.rs` — задаёт `selected_tools: Some(vec!["python_repl"])` явно (строки 85, 547) → **добавление tools не сломает** (подтверждает `design-matrix.md §13.1`).
- proptest уже в workspace (`Cargo.toml`, dev-dep) — используется в `pa-daemon/src/supervisor/tests.rs:2389`, `pa-tui/src/view/geometry.rs:378`.
- `crates/pa-edit-src-pi-edit/tests/` — 12 тест-файлов, включая `hashline_parity.rs`, `replace_parity.rs`, `apply_patch_parity.rs` (parity с TS-реализацией oh-my-pi).
- 111 тест-файлов всего; `pa-daemon/tests/` — ~70 e2e.

### 7.1 Стратегия по слоям

| Слой | Стратегия | Обоснование |
|---|---|---|
| **Контракты** (Ось 1) | schema-snapshot тесты (JSON-schema → golden) | как `golden_schema_group_matches_ts` |
| **LSP-клиент** (L1-L20) | e2e с реальным сервером (rust-analyzer, pyright, gopls) + mock-сервер | `native-rust-lsp-plan.md`: серверы уже установлены |
| **Граф** (G1-G18) | property-based (инварианты графа) + golden-индекс | proptest уже есть |
| **Инкрементальность** (G7) | **дифференциальный** тест: full-rebuild vs incremental на случайных правках → граф обязан совпасть | ключевой инвариант |
| **Кэши** (M10/M11/L11) | инвалидация: изменить контент → кэш обязан промахнуться | M11 witness-keyed |
| **Guards** (E3/E4/E5/R4) | red-first: каждый guard → фикстура-нарушитель | `pa-edit-src` parity-тесты как образец |
| **Parity с TS** | golden-replay (default-off → байт-идентичность) | `design-matrix.md §13.3` |

### 7.2 Как тестировать инкрементальность (главный тест)

**Property-based инвариант (предлагается):**
```
для случайной последовательности правок [p1..pn]:
  graph_incremental = apply(G7, p1..pn)
  graph_full        = rebuild(p1..pn)
  assert graph_incremental.nodes == graph_full.nodes
  assert graph_incremental.edges == graph_full.edges
```
Это ловит все дыры 3-A..3-D разом. В матрице **нет** такой фишки-теста. `R9` note («Готовый механизм closure-repair») + `codebase-memory-deep.md` («каждая неопределённость → полный ребилд») дают основу.

### 7.3 Как тестировать кэши

| Кэш | Тест |
|---|---|
| L11 surface | изменить тело → surface_sha идентичен → инвалидация НЕ происходит; изменить defs → инвалидация происходит |
| M10 query semantic | запрос A → ответ; похожий запрос B → hit; **изменить граф → промах** (дыра 3-F) |
| M11 witness | сменить generation → старый witness не находится; refcount→0 → GC |
| O3 parse | collision хэша → re-parse (verify-by-bytes) |

### 7.4 Как тестировать guards

Red-first фикстуры (по `oh-my-pi-deep.md §2.1`):
- E3 stale-tag: подать устаревший tag → ожидать mismatch-diagnostics (различает «файл изменился» vs «tag не из сессии»).
- E4 seen-lines: anchor вне displayed set → ожидать reveal ≤40×512 + запись в seen-set + ошибку; retry проходит.
- E4 shift-rescue: сдвиг ≥8 символов evidence → rescue; pure insert → нет.
- E5 noop: 3 byte-identical → hard stop.
- R4 staged: hash совпал, revision нет → `BindingMismatch` hard reject.

### 7.5 Что тестировать первым по фазам

| Фаза | Приоритетный тест |
|---|---|
| 1 (pa-lsp) | e2e: rust-analyzer definition/references/hover на фикстуре; diagnostics-ledger дедуп |
| 2 (pa-ast+diff) | parity: `pi-diff` vs jsdiff (golden); `pi-ast::block_range_at` на языках |
| 3 (pa-graph disk) | schema-migration + FTS5 camelCase-split поиск |
| 4 (pa-graph ram) | — (отложить) |
| 5 (closure-repair) | **дифференциальный full-vs-incremental** (см. 7.2) |
| 6 (search) | recall@k на golden-запросах; RRF vs BM25-only |
| 7 (lsp-surface+changes) | diagnostics-after-edit: до/после, только новые; detect_changes на git-diff |
| 7.5 (memory) | eviction не теряет живые узлы; coherence не вытесняет hot |
| 8 (edit-hybrid) | guards red-first; parity с Python-фасадом edit |
| 9 (optional) | — |

### 7.6 Фаззинг графа (предлагается)

- Генерация случайных кодбаз (структур) → индексация → проверка инвариантов (нет висячих рёбер, нет дублей по `UNIQUE(source,target,type,local_name_gen)`).
- Генерация случайных правок + конкурентных запросов → race-детект (WAL, R4 binding).
- Фаззинг name_path: имена с `/`, `[]`, юникод → матчер не падает.

---

## ТОП-10 КРИТИЧНЫХ НАХОДОК (что исправить в матрице)

| # | Находка | ID фишек | Тип | Что делать |
|---|---|---|---|---|
| **1** | **63 цикла в `intersects[]`; тип ребра не различается.** Порядок реализации невыводим. | G1/G2/G3/V1/V2/V12/E1/E2/M1/M2/L1/L2/L4/L8 | конфликт | Ввести `intersects_kind: depends_on\|shares_api\|feeds`; перестроить фазовый план |
| **2** | **Нет контракта host_request kinds.** Цепочка edit→LSP→graph держится на 1 упоминании `did_change`. | L10, E6, L4 | пробел | Зафиксировать 5 kind-ов + `FileChangeEvent` (Ось 1.2) |
| **3** | **`DiagnosticIdentity` противоречива:** L8 (без range) vs L9/E6 (с range). В коде — Serena-вариант. | L8, L9, E6 | конфликт | Выбрать Serena-вариант (уже в `ledger.rs`), переформулировать L8 |
| **4** | **LSP-поверхность ломает «чистую функцию per-file» closure-repair** (недетерминизм). | G7 ∩ L11 | риск | Базировать surface на pa-ast; LSP — override (как решено в `architecture-synthesis-model.md §15.10`, но не в матрице) |
| **5** | **`name_path` (Serena) vs `qualified_name` (граф) — не определено их тождество.** Ключ к связке LSP↔graph. | L3, L6, G3, G4, S10 | пробел | Определить адаптер name_path↔qn или единый ключ |
| **6** | **Двойной путь записи в файл:** `rename_symbol` (L5) пишет через `apply_workspace_edit`, минуя guards (E3/E4/R4). | L5 vs E3/E4/R4 | риск | Направить rename через EditWriter/staged-контур |
| **7** | **20 нарушений фазового порядка + 96 фишек без фазы.** Фазовый план покрывает ~32%. | L4→L9, G2→G1, O2→E2, L10→E1… | конфликт | Пересобрать фазы по `depends_on`-графу; включить S-серию |
| **8** | **`generation` — 2 разные сущности** (LSP document-version vs graph index-version). | L4/L13, meta observability | коллизия | Развести: `doc_generation` (LSP) vs `graph_generation` |
| **9** | **Нет provenance для seen-lines guard (E4).** Read идёт через REPL/fs — «displayed set» не собирается. | E4, O4 | пробел | Явный provenance-канал из read/search в edit-store |
| **10** | **51 фишка P3 (36%), 12 high-cost; дубликаты (M6≈R17, V1≈V12/G12).** Переобогащение. | M6/R17, V1/V12/G12, P1-P8 | риск | Backlog; дедуп; фокус на 30-40 фишек |

**Дополнительно (11-15):** 1 dangling ID (`G10→M-harness`); формат surface-JSON не зафиксирован (а `preserve_order` усложняет канонизацию); `WorkspaceEdit` file-ops отвергнуты (L18/L19 несовместимы); `query-cache` M10∩M11 не описан (устаревшие ответы); 3 разных смысла `coherence`.

---

## ТОП-10 ПРИОРИТЕТНЫХ ТЕСТОВ/МЕТРИК

| # | Тест / метрика | Что покрывает | Где база |
|---|---|---|---|
| **1** | **Дифференциальный тест closure-repair**: full-rebuild == incremental (property-based, случайные правки) | G7, R8, R9, L11 | proptest в workspace; `codebase-memory-deep.md` приём №1 |
| **2** | **e2e LSP с реальным rust-analyzer/pyright/gopls**: definition/references/hover/rename | L1,L2,L5,L6,S10,S11 | серверы установлены (`native-rust-lsp-plan.md`) |
| **3** | **Метрика `graph_recall_at_k` + `search_mrr` на golden-запросах** | G3, V2, V14, G13 | golden-corpus шаблон |
| **4** | **Инвалидация кэшей**: L11 (тело vs defs), M10 (изменение графа→промах), M11 (generation drift) | L11,M10,M11,O3 | дыра 3-F |
| **5** | **Red-first guards**: E3 stale-tag, E4 seen-lines+shift-rescue, E5 noop=3, R4 binding-mismatch | E3,E4,E5,R4 | `pa-edit-src-pi-edit/tests/hashline_parity.rs` |
| **6** | **Метрика `graph_edge_precision/recall`** (tree-sitter CALLS vs LSP-override) | G8, G2, L6 | codebase-memory LSP-override floor 0.6 |
| **7** | **Parity golden-replay при default-off**: байт-идентичность upstream | все (изоляция) | `golden_replay.rs`, `prompt_guards.rs:85,547` |
| **8** | **Метрика `graph_full_rebuild_count` + `graph_closure_size`** — детект деградации closure-repair | G7 | Ось 6.3 |
| **9** | **diagnostics-after-edit**: snapshot до/после, только новые; identity-стабильность при неизменном файле | E6,L4,L8,L9,L13 | Serena `DiagnosticsDiff` |
| **10** | **Property-based name_path**: имена с `/`, `[N]`, юникод, generics → матчер не падает; round-trip parse→render | L3,S10,G4 | `symbols.rs` тесты есть — расширить |

---

## ИТОГОВЫЙ ВЕРДИКТ

Матрица **богата по идеям** (142 фишки, 5 источников) и **точна по точкам интеграции** (engine.rs:446, types.rs:498, shared.rs:270, Cargo.toml:49 — все верифицированы по коду). Слабые места — **не идеи, а связность**: контракты между фишками не выписаны, граф зависимостей цикличен и не типизирован, семантика терминов из 5 источников конфликтует.

**Порядок исправления:**
1. Типизировать `intersects` (Ось 2) — разблокирует фазовый план.
2. Выписать контракты: host_request kinds, DB-DDL, surface-JSON, settings-структуры (Ось 1).
3. Развести терминологические коллизии (Ось 4): generation, witness, coherence, repair, name_path↔qn.
4. Зафиксировать differential-тест closure-repair (Ось 7.2) — он верифицирует всю инкрементальность.
5. Сократить P3-бэклог и дедуплицировать (Ось 5.4, Топ-10 #10).

**Критический путь MVP остаётся верным** (`design-matrix.md §14`): `pa-lsp (L1,L2,L4,L6)` + `pa-ast (G2)` + `pa-graph (G1,G3,G4)` + `E6` — 80% ценности за ~30% работы. Но без контрактов (Ось 1) и типизации зависимостей (Ось 2) он не собирается.
