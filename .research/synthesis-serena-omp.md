# СИНТЕЗ: Serena (полный разбор) + oh-my-pi (LSP) → нативный Rust в форке

## ПОЛНЫЙ инвентарь Serena (изучено)

### 1. Tool-модули (2 350 LOC всего)
| Файл | LOC | Ключевые tools |
|---|---|---|
| `tools_base.py` | 587 | `Tool`, маркеры, `EditingToolWithDiagnostics`, `ToolRegistry` |
| `symbol_tools.py` | 468 | `GetSymbolsOverview`, `FindSymbol`, `FindReferencingSymbols`, `FindImplementations`, `FindDeclaration`, `GetDiagnosticsForFile`, `GetDiagnosticsForSymbol`, `ReplaceSymbolBody`, `InsertAfterSymbol`, `InsertBeforeSymbol`, `RenameSymbol`, `SafeDeleteSymbol`, `RestartLanguageServer` |
| `jetbrains_tools.py` | 463 | JetBrains-бэкенд (не нужен — у нас LSP) |
| `file_tools.py` | 362 | `ReadFile`, `CreateTextFile`, `ListDir`, `FindFile`, `ReplaceContent`, `ReplaceInFiles`, `DeleteLines`, `ReplaceLines`, `InsertAtLine`, `SearchForPattern` |
| `memory_tools.py` | 122 | `WriteMemory`, `ReadMemory`, `ListMemories`, `DeleteMemory`, `RenameMemory`, `EditMemory` |
| `config_tools.py` | 82 | `OpenDashboard`, `ActivateProject`, `RemoveProject`, `GetCurrentConfig` |
| `query_project_tools.py` | 78 | `ListQueryableProjects`, `QueryProject` |
| `repl_tools.py` | 64 | `SerenaRepl` (beta) |
| `workflow_tools.py` | 57 | `Onboarding`, `InitialInstructions`, `SerenaInfo` |
| `cmd_tools.py` | 54 | `ExecuteShellCommand` |

### 2. ЯДРО адресации: `symbol.py` (1 383 LOC)
- **`NamePathMatcher`** — сердце Serena. Формы паттерна:
  - `method` — простое имя (совпадение по любому символу)
  - `class/method` — relative (suffix-match по name path)
  - `/class/method` — absolute (точное совпадение полного пути)
  - `MyClass/my_method[1]` — overload-индекс
  - substring-режим для последнего сегмента
- **`LanguageServerSymbol`** — обёртка над `UnifiedSymbolInformation`: `name_path`, `body`,
  `get_body_start_position`/`end_position`, `iter_name_path_components_reversed`, `overload_idx`
- **`LanguageServerSymbolRetriever`** (581+ LOC) — 30+ методов:
  `find`, `find_unique`, `find_by_location`, `find_referencing_symbols`,
  `find_implementing_symbols`, `find_declaration`, `get_file_diagnostics`,
  `find_diagnostic_owner_symbol`, `get_symbol_diagnostics`, `get_symbol_overview`,
  `request_info_for_symbol(_batch)` (hover-инфо)

### 3. Модель диагностик: `lsp/lsp_diagnostics.py` (259 LOC)
- **`DiagnosticIdentity`** — идентичность диагностики (message + range + severity + code + source)
- **`PublishedDiagnosticsSnapshot`** — снапшот ДО правки (generation + identities)
- **`DiagnosticsDiff`** — считает только НОВЫЕ диагностики после правки
- **`GroupedDiagnostics`** — группировка `file → severity → name_path → [diagnostic]`
  (привязка диагностики к владеющему СИМВОЛУ!)
- **ВАЖНО**: `DiagnosticsContext.ENABLE_DIAGNOSTICS_DEFAULT = False` — Serena сама считает
  per-edit diagnostics «questionable» (правки часто намеренно вносят ошибки).
  Включается только явно. Урок: не навязывать авто-диагностику после каждой правки.

### 4. Управление серверами: `ls_manager.py` (385 LOC) + `solidlsp` (81 сервер!)
- `LanguageServerFactory` — создаёт LS по id
- `LanguageServerManager` — `from_languages` (параллельный старт через threads),
  `get_language_server(relative_path)`, `restart`, `add`/`remove`, `stop_all`, `sync_file_system_changes`
- `LanguageServerFileChangeNotifier` — polling изменений FS → notify серверов
- **`LanguageServerId` enum** — 60+ языков (csharp, python, rust, java, kotlin, typescript,
  go, ruby, dart, cpp, php, r, perl, clojure, elixir, elm, terraform, swift, bash, crystal,
  cue, zig, lua, luau, nix, erlang, ocaml, al, fsharp, rego, scala, julia, fortran, haskell,
  haxe, lean4, groovy, vue, svelte, astro, powershell, pascal, matlab, msl, bsl, ada,
  gdscript, qml, gleam, nextflow, wolfram + варианты)
- **`SolidLanguageServer`** (3 295 LOC!) — главный клиент:
  `request_definition`, `request_implementation`, `request_references`,
  `request_document_symbols`, `request_full_symbol_tree`, `request_dir_overview`,
  `request_hover`, `request_signature_help`, `request_referencing_symbols`,
  `request_containing_symbol`, `request_symbol_at_location`, `request_defining_symbol`,
  `request_implementing_symbols`, `request_workspace_symbol`, `request_rename_symbol_edit`,
  `request_text_document_diagnostics` (pull), `request_published_text_document_diagnostics`
- **`LanguageServerConfig`** — workspace_folders, additional_workspace_folders,
  ignored_paths (gitignore-синтаксис), encoding

### 5. Memories (`memories/`, 1 231 LOC)
- `memory_manager.py` (450) + `memory_reference_analysis.py` (781)
- md-файлы, `mem:`-ссылки между ними, `global/`-префикс
- **Дублирует continual harness prime-agent** → НЕ переносим.

## Что берём у кого (финальный синтез)

| Идея | Источник | Что именно |
|---|---|---|
| Единый action-tool | omp | один tool `lsp` с `action` |
| Таблица серверов | omp + Serena | command/args/fileTypes/rootMarkers (omp), расширить до Serena-списка |
| **name_path-адресация** | **Serena** | `Class/method`, `/abs/path`, `[overload]`, substring |
| **diagnostics-к-символу** | **Serena** | `GroupedDiagnostics`: file→severity→name_path |
| **new-vs-existing diff** | **Serena** | `DiagnosticIdentity` + snapshot до/после |
| Diagnostics-ledger | omp | дедупликация (проще Serena-варианта) |
| readonly/write tier | omp | `LSP_READONLY_ACTIONS` |
| pull-diagnostics | Serena | `textDocument/diagnostic` (pull) + fallback на publish |
| idle/timeout клиентов | omp | idle-checker |
| FS-change notifier | Serena | polling → `didChangeWatchedFiles` |

## Ключевые уроки Serena (что НЕ копировать слепо)
1. **Не навязывать диагностику после каждой правки** — Serena держит это off по умолчанию.
2. **Диагностику привязывать к символу** (`name_path`), а не только к файлу — это killer-feature.
3. **Сравнивать до/после** (`DiagnosticIdentity`), показывать только новые.
4. **Memory-система не нужна** — у prime-agent есть continual harness.
5. **JetBrains-бэкенд не нужен** — только LSP.

## Реализация: расширенный `pa-lsp`
Новый крейт получает:
- `tool.rs` — `LspTool` (10 actions, синтез omp-схемы)
- `manager.rs` — `LspManager` (per-session клиенты, синтез omp-mux + Serena-ls_manager)
- `client.rs` — `LspClient` (async-lsp, синтез omp-client + Serena-SolidLanguageServer)
- `symbols.rs` — **name_path resolution** (порт `NamePathMatcher` Serena на Rust)
- `ledger.rs` — diagnostics diff (синтез Serena-`DiagnosticsDiff` + omp-`DiagnosticsLedger`)
- `servers.rs` — таблица серверов (расширенная)
- `types.rs` — action/params (готово)
