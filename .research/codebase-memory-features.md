# codebase-memory-mcp: разбор фич и что переносимо в prime-agent/allspark

## Общее
- Бинарь 300 MB, ELF, статический, not stripped. Язык: Rust (собственный движок).
- Хранилище: **SQLite** (`/root/.cache/codebase-memory-mcp/<project>.db`), WAL.
- Один .db на проект; `prime-agent.db` = 157 MB.
- Профили: `--tool-profile=analysis|scout` (ограничивает surface).

## 15 MCP-инструментов
| Tool | Назначение | Реализуемо у нас? |
|---|---|---|
| `index_repository` | индексация репо (mode full/incremental) | частично (нужен парсер) |
| `index_status` | статус индекса | да |
| `list_projects` | список проектов | да |
| `delete_project` | удалить | да |
| `search_graph` | BM25 + regex + **semantic_query** по символам | **да** |
| `search_code` | graph-ranked текст (compact/full/files) | **да** |
| `trace_path` | callers/callees, data_flow, cross_service | **да** |
| `detect_changes` | git-diff → затронутые символы | **да** |
| `query_graph` | read-only Cypher | сложно (нужен Cypher-движок) |
| `get_graph_schema` | labels/edges counts | да |
| `get_code_snippet` | тело символа | **да** |
| `get_architecture` | overview/structure/layers/clusters/cycles | частично |
| `check_index_coverage` | coverage/freshness | да |
| `manage_adr` | ADR-память (outline/get/update/set_sections) | **да** |
| `ingest_traces` | runtime-трейсы (валидация; edge-creation НЕ реализован) | нет смысла |

## Схема БД (ключевое)
```
nodes(id, project, label, name, qualified_name, file_path, start_line, end_line, properties)
  UNIQUE(project, qualified_name)
edges(id, project, source_id, target_id, type, properties,
      url_path_gen GENERATED, local_name_gen GENERATED)
  UNIQUE(source_id, target_id, type, local_name_gen)
nodes_fts   FTS5(name, qualified_name, label, file_path, body, unicode61)
node_vectors(node_id, project, vector BLOB)   -- 192-dim int8!
token_vectors(id, project, token, vector BLOB, idf)  -- 192-dim int8
lsp_surface(project, rel_path, surface_sha, defs_json, ref_bloom, config_ctx)
file_hashes(project, rel_path, sha256, mtime_ns, size)
index_coverage(project, rel_path, kind, detail)
project_summaries(project, summary, source_hash, ...)  -- ADR
projects(name, indexed_at, root_path)
store_meta(k, v)
```

## Labels (19): Function, Field, Method, Variable, Struct, File, Module, Enum, Class,
Folder, Section, Type, EnvVar, Interface, Route, Decorator, Channel, Project, Branch

## Edge types (25): USAGE, CALLS, DEFINES, DECORATES, DEFINES_METHOD, WRITES,
SIMILAR_TO, IMPORTS, CONTAINS_FILE, TESTS, OVERRIDE, CONFIGURES, CONTAINS_FOLDER,
CALL_REFERENCE, IMPLEMENTS, SEMANTICALLY_RELATED, FILE_CHANGES_WITH, HTTP_CALLS,
INHERITS, RAISES, TESTS_FILE, EMITS, THROWS, LISTENS_ON, HAS_BRANCH

## Богатые properties рёбер
- **CALLS**: `{callee, confidence(0..1), strategy(same_module|unique_name), candidates, args:[{i,e}], line}`
  → это и есть DATA-FLOW: `args` с индексами и выражениями!
- **SIMILAR_TO**: `{jaccard, same_file}`
- **SEMANTICALLY_RELATED**: `{score, same_file}`
- **HTTP_CALLS**: `{callee, url_path, via(arg_url)}` — cross-service
- **DECORATES**: `{decorator}`
- **IMPORTS**: `{local_name}`
- **HAS_BRANCH**: `{branch, head_sha, base_sha, is_git, ...}`

## Ключевые фичи (по убыванию ценности для нас)

### 1. Векторный semantic search — int8-квантизация (768 dim)
- `node_vectors`: **768-мерный int8** (768 байт/вектор, значения в диапазоне ~-81..24).
- `token_vectors`: та же размерность (768 i8) + **IDF** (BM25-стиль).
- `token_vectors`: та же размерность + **IDF** (BM25-стиль).
- `search_graph(semantic_query=[...])` — поиск по ключевым словам через усреднённый
  токен-вектор.
- **Ценность**: семантический поиск символов без эмбеддинг-сервера (всё локально).

### 2. FTS5 BM25 — `nodes_fts`
- Полнотекстовый поиск по name/qualified_name/label/file_path/body.
- `search_graph(query=...)` = BM25 по этой таблице.

### 3. `lsp_surface` — кэш LSP-поверхности ⭐ (прямо пересекается с pa-lsp!)
- На каждый файл: `surface_sha` (хэш), `defs_json` (defs + reg), `ref_bloom` (bloom-фильтр ссылок).
- `defs_json.lsp[]`: `{qn, sn(name), lb(label), dm(module), ret, emb(embedded?), fd, mn, spt,
  ii(is_impl?), lg(line?), tq, ir, ab, dec}` — компактная запись символов.
- `defs_json.reg[]`: `{n(name), q(qualified), k(kind)}` — реестр.
- **Ценность**: переиспользовать кэш вместо переспроса LSP на каждый вызов.
- **Пересечение**: наш `pa-lsp` мог бы писать в такую же таблицу.

### 4. CALLS с confidence + strategy
- `confidence: 0.75|0.90`, `strategy: unique_name|same_module`, `candidates: N`.
- Разрешение вызовов честное: при неоднозначности confidence падает.

### 5. detect_changes — git-diff → символы
- `scope: files|impact`, `direction: inbound|outbound|both`, `depth`.
- Использует `file_hashes` (sha256) + `FILE_CHANGES_WITH` + `HAS_BRANCH` (git-состояние).

### 6. manage_adr — ADR-память в `project_summaries`
- `outline` (заголовки), `get`, `update` (заменить целиком),
  `set_sections` (заменить только указанные секции — **идемпотентно**).
- **Ценность**: архитектурные решения рядом с графом.

### 7. check_index_coverage — честность индекса
- `index_coverage(rel_path, kind, detail)`: какие файлы parse_partial/skipped.
- «Clean — не доказательство полноты» (эпистемическая честность).
- `store_meta.coverage_shadow_fp` — fingerprint.

### 8. get_architecture — аспекты
- `overview|structure|dependencies|routes|languages|packages|entry_points|hotspots|
  boundaries|layers|file_tree|clusters|cycles`.
- layers/clusters/cycles — графовые алгоритмы (SCC, community detection).

## Что ПЕРЕНОСИМО в allspark (по ценности)

| Фича | Сложность | Куда | Вердикт |
|---|---|---|---|
| **LSP-поверхность + кэш** (`lsp_surface`) | средняя | `pa-lsp` | ⭐ ПЕРЕНОСИМ |
| **BM25 FTS по символам** | средняя | новый `pa-graph` | ⭐ ПЕРЕНОСИМ |
| **semantic search (768-dim int8 vectors)** | высокая | `pa-graph` | опционально |
| **CALLS confidence/strategy** | низкая | `pa-lsp`/`pa-graph` | ⭐ ПЕРЕНОСИМ |
| **ADR (manage_adr)** | низкая | skill/settings | ⭐ ПЕРЕНОСИМ (просто) |
| **detect_changes (git→символы)** | средняя | `pa-graph` | ПЕРЕНОСИМ |
| **coverage-честность** | низкая | `pa-graph` | ПЕРЕНОСИМ |
| Cypher-движок | очень высокая | — | НЕ переносим |
| ingest_traces | — | — | НЕ переносим (не реализован даже там) |
| Полная индексация (tree-sitter) | высокая | — | у нас есть pi-ast |

## Синтез трёх систем (обновление)
```
Serena      → symbol tools, name_path, diagnostics-to-symbol
oh-my-pi    → LSP client, action-tool, ledger, writethrough
codebase-memory → BM25/semantic search, LSP-surface cache, CALLS-confidence,
                  ADR, coverage-honesty, detect_changes
prime-agent → python_repl (PTC), native AgentTool, kernel/skills
```

### Итоговая архитектура (нативный Rust)
1. `pa-lsp` — LSP-клиент (пишу сейчас): spawn/init/query + name_path + ledger.
2. `pa-graph` (новый) — SQLite-граф: nodes/edges/FTS/vectors/lsp_surface,
   BM25 + semantic search, CALLS-confidence, detect_changes, coverage, ADR.
3. `pa-ast` (из pi-ast) — tree-sitter для индексации (DEFINES/CALLS).
4. Нативные tools: `lsp` (готов), `graph_search`, `graph_trace`, `graph_changes`.
