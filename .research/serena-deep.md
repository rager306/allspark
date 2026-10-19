# Serena Deep-Dive: инвентарь фишек/приёмов для форка prime-agent (pa-lsp/pa-graph)

> Источник: `/tmp/serena` (Serena MCP v2.0.1.dev0, commit f9972a3, read-only).
> Цель — извлечение ДИЗАЙН-паттернов (не порт кода) для нативного Rust-аналога.
> Все ссылки — `файл:строка` в `/tmp/serena`.

---

## 0. Карта кода (LOC, подтверждено)

| Модуль | LOC | Роль |
|---|---|---|
| `src/serena/symbol.py` | 1383 | Ядро адресации: NamePathMatcher, LanguageServerSymbol, Retriever |
| `src/serena/ls_manager.py` | 385 | Менеджер LSP-серверов + file-change notifier |
| `src/serena/project.py` | 784 | Активация проекта, ignore-spec, LS-менеджер, sync |
| `src/serena/agent.py` | 1713 | Оркестратор: toolset, prompt params, facades, sessions |
| `src/serena/code_editor.py` | 563 | Symbol-aware редактор (body replace/insert/delete/rename) |
| `src/serena/tools/tools_base.py` | 587 | Tool, маркеры, EditingToolWithDiagnostics, ToolRegistry |
| `src/serena/lsp/lsp_diagnostics.py` | 259 | Модель диагностик + diff до/после |
| `src/serena/lsp/lsp_backend.py` | 80 | LSP LanguageBackend (фасады, code editor) |
| `src/serena/repl/facade.py` | 759 | Facade/FacadeMethod/ApiScope/ReferencedType |
| `src/serena/repl/repl.py` | 384 | REPL-исполнитель + SerenaReplEntrypoint + s.info() |
| `src/serena/repl/representable.py` | 106 | Representable/Renderer + JsonObject |
| `src/serena/repl/api/lsp_api.py` | 791 | LSP-фасад (10 методов) + рендереры |
| `src/serena/repl/api/jb_api.py` | 642 | JetBrains-фасад (14 методов) |
| `src/serena/memories/memory_manager.py` | 450 | Memories (md, mem:-ссылки, global/) |
| `src/serena/memories/memory_reference_analysis.py` | 781 | Referential integrity, autofix |
| `src/solidlsp/ls.py` | 3295 | SolidLanguageServer — главный LSP-клиент |
| `src/solidlsp/ls_config.py` | 1221 | LanguageServerId enum (77 значений!), config |
| `src/solidlsp/ls_process.py` | 847 | LSP transport (stdio/socket), JSON-RPC |

Итого tools: **53 конкретных Tool-класса** (29 default + 24 optional), распределены по 9 модулям.
REPL: **65 `@facade_method`** в 8 фасадах.

---

## 1. ПОЛНАЯ ТАБЛИЦА TOOLS (53)

Маркеры: `C`=CanEdit, `SR`=SymbolicRead, `SE`=SymbolicEdit, `Opt`=Optional, `Beta`, `NoProj`=DoesNotRequireActiveProject.
«Применимость» — оценка для форка prime-agent (Y=высокая, ~=средняя, N=не нужна).

| Tool | Модуль | Назначение | Ключевые параметры | Маркеры | Прим. |
|---|---|---|---|---|---|
| `get_symbols_overview` | symbol_tools | Top-level символы файла | relative_path, depth, max_answer_chars | SR | Y |
| `find_symbol` | symbol_tools | Поиск символов по name_path | name_path_pattern, depth, relative_path, include_body, include_info, include_kinds, exclude_kinds, substring_matching, max_matches, max_answer_chars | SR | Y |
| `find_referencing_symbols` | symbol_tools | Кто ссылается на символ | name_path, relative_path, include_kinds, exclude_kinds, max_answer_chars | SR | Y |
| `find_implementations` | symbol_tools | Реализации символа | name_path, relative_path, include_info, include_kinds, exclude_kinds, max_answer_chars | SR | Y |
| `find_declaration` | symbol_tools | Declaration через regex-наведение | relative_path, regex, containing_symbol_name_path, include_body, include_info | SR | Y |
| `get_diagnostics_for_file` | symbol_tools | Диагностики файла, сгруппированы по символу | relative_path, start_line, end_line, min_severity, max_answer_chars | SR | Y |
| `get_diagnostics_for_symbol` | symbol_tools | Диагностики символа (+references) | name_path, reference_file, check_symbol_references, min_severity, max_answer_chars | SR, Opt | Y |
| `replace_symbol_body` | symbol_tools | Замена тела символа | name_path, relative_path, body | C(+diag) | Y |
| `insert_after_symbol` | symbol_tools | Вставка после символа | name_path, relative_path, body | C(+diag) | Y |
| `insert_before_symbol` | symbol_tools | Вставка перед символом | name_path, relative_path, body | C(+diag) | Y |
| `rename_symbol` | symbol_tools | LSP-rename по всему коду | name_path, relative_path, new_name | SE | Y |
| `safe_delete_symbol` | symbol_tools | Удаление, если нет ссылок | name_path_pattern, relative_path | SE | Y |
| `restart_language_server` | symbol_tools | Рестарт LS | — | Opt | Y |
| `read_file` | file_tools | Чтение файла/диапазона | relative_path, start_line, end_line, max_answer_chars | — | Y |
| `create_text_file` | file_tools | Создание/перезапись файла | relative_path, content | C(+diag) | Y |
| `list_dir` | file_tools | Листинг (рекурсивно) | relative_path, recursive, skip_ignored_files, max_answer_chars | — | Y |
| `find_file` | file_tools | Поиск по маске файла | file_mask, relative_path, skip_ignored_files | — | Y |
| `replace_content` | file_tools | Замена по literal/regex (с wildcards) | relative_path, needle, repl, mode, allow_multiple_occurrences | C(+diag) | Y |
| `replace_in_files` | file_tools | Массовая замена с dry-run+occurrence ids | needle, repl, mode, relative_path, paths_include_glob, paths_exclude_glob, dry_run, occurrence_ids, expected_count, max_answer_chars | C(+diag) | Y |
| `search_for_pattern` | file_tools | Regex-поиск по проекту | substring_pattern, context_lines_before/after, globs, relative_path, restrict_search_to_code_files, skip_ignored_files, multiline, max_answer_chars | — | Y |
| `delete_lines` | file_tools | Удаление строк | relative_path, start_line, end_line | C+Opt | ~ |
| `replace_lines` | file_tools | Замена строк | relative_path, start_line, end_line, content | C+Opt | ~ |
| `insert_at_line` | file_tools | Вставка на строке | relative_path, line, content | C+Opt | ~ |
| `write_memory` | memory_tools | Запись памяти (md) | memory_name, content, max_chars | C | ~ |
| `read_memory` | memory_tools | Чтение памяти | memory_name | — | ~ |
| `list_memories` | memory_tools | Список (фильтр topic) | topic | — | ~ |
| `edit_memory` | memory_tools | Regex-замена в памяти | memory_name, needle, repl, mode, allow_multiple_occurrences | C | ~ |
| `rename_memory` | memory_tools | Переименование + фикс mem:-ссылок | old_name, new_name | C | ~ |
| `delete_memory` | memory_tools | Удаление памяти | memory_name | C | ~ |
| `activate_project` | config_tools | Активация проекта | project, session_id | NoProj | ~ |
| `remove_project` | config_tools | Удаление проекта из конфига | project_name | NoProj+Opt | N |
| `get_current_config` | config_tools | Текущая конфигурация | — | — | ~ |
| `open_dashboard` | config_tools | Открытие веб-дашборда | — | Opt+NoProj | N |
| `onboarding` | workflow_tools | Онбординг проекта | — | C | ~ |
| `initial_instructions` | workflow_tools | «Instructions Manual» | — | NoProj | Y |
| `serena_info` | workflow_tools | Прогрессивная док-ция (topic) | topic | Opt+NoProj | Y |
| `execute_shell_command` | cmd_tools | Выполнение shell-команды | command, cwd, capture_stderr, max_answer_chars | C | Y |
| `serena_repl` | repl_tools | Выполнение Python через фасады | session_id, code | Opt+Beta+C | Y |
| `list_queryable_projects` | query_project_tools | Список внешних проектов | symbol_access | Opt+NoProj | ~ |
| `query_project` | query_project_tools | Запрос read-only tool во внешнем проекте | project_name, tool_name, tool_params_json | Opt+NoProj | ~ |
| `jet_brains_*` (13 шт.) | jetbrains_tools | JetBrains-бэкенд (find_symbol/move/safe_delete/inline/refs/overview/type_hierarchy/find_declaration/find_implementations/rename/debug/run_inspections/list_inspections) | — | SR/SE+Opt | N (у нас LSP) |

### Выводы по tools
- **29 tools default-enabled, 24 optional** (issubclass ToolMarkerOptional, tools_base.py:87). Beta — только `serena_repl` (repl_tools.py:12).
- Имена выводятся из класса: `FindSymbolTool` → `find_symbol` (tools_base.py:172-180, `get_name_from_cls`: снять `Tool`, camel→snake).
- `ToolRegistry` (tools_base.py:477) авто-регистрирует через `iter_subclasses(Tool, inclusion_predicate=lambda c: "apply" in c.__dict__)` (только конкретные), фильтруя по `tool_packages=["serena.tools"]` (tools_base.py:473,491). Дубликат имени → ValueError.
- `_deleted_tools` (tools_base.py:478-485) — tombstone-список удалённых имён для мягкой обработки устаревших конфигов.
- `get_param_aliases` (symbol_tools.py:151) — обратная совместимость (`name_path`→`name_path_pattern`).
- `LEGACY_TOOL_NAME_MAPPING = {"replace_regex": "replace_content"}` (agent.py:130).

---

## 2. symbol.py — полный разбор механики

### 2.1. NamePathMatcher (symbol.py:145-219) — ядро адресации
Формы паттерна (док, symbol.py:145-158):
- `method` — простое имя (совпадение по любому символу);
- `class/method` — **relative** (suffix-match по name_path);
- `/class/method` — **absolute** (точное совпадение полного пути в файле);
- `MyClass/my_method[1]` — **overload-индекс**;
- substring-режим для последнего сегмента.

Алгоритм `matches_reversed_components` (symbol.py:203-219):
1. Паттерн разбивается на компоненты; `_is_absolute_pattern = pattern.startswith("/")` (symbol.py:192);
2. Сравнение идёт **с конца** (reversed) — поэтому `class/method` матчит суффикс;
3. substring применяется только к **последнему** сегменту (`use_substring_matching = self._substring_matching and (i == 0)`, symbol.py:209);
4. Для absolute-паттерна после исчерпания компонентов требуется, чтобы у символа НЕ осталось предков (symbol.py:212-217).

`PatternComponent.from_string` (symbol.py:160-171) парсит `name[idx]`, где `idx` — digit.
`NamePathComponent.__repr__` (symbol.py:139-142) рендерит `name[overload_idx]`.

### 2.2. LanguageServerSymbol (symbol.py:222-550) — обёртка над UnifiedSymbolInformation
- `name`, `symbol_kind`, `symbol_kind_name` (symbol.py:240-245);
- `is_low_level()` (symbol.py:247) = `not SymbolKind.is_container(kind)` — переменные/константы не показываются в overview;
- `overload_idx` (symbol.py:255) — из `symbol_root.get("overload_idx")` (заполняется LS-слоем);
- `body` (symbol.py:342) — из `symbol_root["body"].get_text()` (лениво из SymbolBody);
- `line`/`column` (symbol.py:322-340) — из `selectionRange.start` (идентификатор), а НЕ из `range`;
- `get_body_line_numbers` (symbol.py:309) — (start_line, end_line) из location.range;
- `get_name_path()` (symbol.py:349) — `"/".join(reversed([str(c) for c in iter_name_path_components_reversed()]))`;
- `iter_name_path_components_reversed` (symbol.py:357) — сам + `iter_ancestors(up_to_symbol_kind=SymbolKind.File)`;
- `iter_ancestors` (symbol.py:366) — идёт по `parent`, СТОП перед первым предком нужного kind (File/Package);
- `find(pattern, include_kinds, exclude_kinds, substring_matching)` (symbol.py:386) — рекурсивный обход subtree с матчером.

### 2.3. to_dict (symbol.py:452-550) — управляемый вывод
Флаги: `name_path, name, kind, location, depth, body, body_location, children_body, children_name_path, children_name, relative_path, child_inclusion_predicate`.
- `depth` — рекурсивная вложенность (0 = без детей);
- `children_body` — включить тело детей (обычно не нужно: тело ребёнка ⊂ тела родителя);
- `relative_path` у детей всегда исключён (symbol.py:530-540).

### 2.4. LanguageServerSymbolRetriever (symbol.py:581-1149) — все методы
| Метод | Строки | Механика |
|---|---|---|
| `request_info_for_symbol` | 622 | hover → docstring/signature (sanitize list/dict/str) |
| `request_info_for_symbol_batch` | 634 | Батч hover с **time budget** (`symbol_info_budget`, default 10s), группировка по файлу, per-file cache (line,col) |
| `find` | 737 | Обход LS (все или один файл); `within_relative_path` → `get_language_server(path)` |
| `find_unique` | 769 | 1 кандидат → ок; 0 → ValueError; >1 → ищет точное совпадение name_path, иначе ValueError со списком |
| `find_by_location` | 801 | Поиск по (relative_path, line, column) через document_symbols |
| `find_referencing_symbols` | 812 | → `_by_location` |
| `find_referencing_symbols_by_location` | 836 | LSP `references` + `include_imports=False, include_self=False, include_file_symbols=True`; фильтр kinds |
| `find_implementing_symbols(_by_location)` | 882/908 | LSP `implementation`; фильтр kinds |
| `find_declaration` | 948 | LSP `definition` в позиции |
| `get_file_diagnostics` | 975 | pull-diagnostics с line range + min_severity |
| `find_diagnostic_owner_symbol` | 1013 | symbol_at_location + `_normalize_symbol_for_diagnostics` (подъём от low-level) |
| `get_symbol_diagnostics(_by_location)` | 1056/1079 | диагностики символа + (опц.) всех referencing-символов |
| `get_symbol_overview` | 1140 | `request_overview` → {path: [symbols]} |

`_symbol_identity` (symbol.py:1000) = `(relative_path, line, column, name_path)` — ключ дедупликации.
`_normalize_symbol_for_diagnostics` (symbol.py:1004) — поднимается по parent, пока `is_low_level()`, чтобы владельцем диагностики был структурный контейнер (функция/класс), а не локальная переменная.

### 2.5. Overload-механика (в solidlsp/ls.py:2077-2094)
При построении `UnifiedSymbolInformation` (`convert_symbols_with_common_parent`):
- считаются имена среди siblings (`total_name_counts`);
- если имя встречается >1 → `usymbol["overload_idx"] = name_counts[name]` (0-based);
- нормализация имён через `_normalize_symbol_name` (ls.py:1942): убирает parameter-lists; **имя не должно содержать `/`** (иначе name_path неразрешим — важное замечание для Rust!).

### 2.6. Body-извлечение: SymbolBody (solidlsp/ls.py:232-311)
- `SymbolBody(lines, start_line, start_col, end_line, end_col)` — **~40 байт** (4 int + ref на lines-буфер!), экономия памяти;
- `get_text()` (ls.py:252): склейка строк, обрезка leading на первой строке и trailing на последней;
- **EOF-конвенция** (ls.py:257-268): range, заканчивающийся на `(len(lines), 0)`, трактуется как «до EOF»; любой другой out-of-range → `InvalidTextLocationError` (не угадывать!);
- `SymbolBodyFactory` (ls.py:289) — разделяет lines-буфер между body одного файла.

---

## 3. Диагностики: модель + diff до/после

Файл `src/serena/lsp/lsp_diagnostics.py` (259 LOC).

### 3.1. DiagnosticIdentity (lsp_diagnostics.py:24-57, `frozen=True`)
Поля: `message, start_line, start_character, end_line, end_character, severity, code_repr, source`.
`_diagnostic_code_repr` (lsp_diagnostics.py:50) — `json.dumps(code, sort_keys=True)` (хэшируемость).
→ Идентичность диагностики = стабильный ключ для сравнения множеств.

### 3.2. PublishedDiagnosticsSnapshot (lsp_diagnostics.py:60-92)
Снимается ДО правки: для каждого файла сохраняет `generation` (из `get_published_diagnostics_generation`) и `set[DiagnosticIdentity]` предупреждений (min_severity=2).

### 3.3. DiagnosticsDiff (lsp_diagnostics.py:151-208)
После правки: запрашивает published diagnostics с `after_generation`, `timeout=2.5`, `min_severity=2`; оставляет только НОВЫЕ идентичности (не входившие в before-snapshot); каждую привязывает к **владеющему символу** через `find_diagnostic_owner_symbol`; fallback `<file>` (lsp_diagnostics.py:191-194).

### 3.4. GroupedDiagnostics (lsp_diagnostics.py:95-149)
Структура: `{relative_path: {severity_name: {name_path: [diagnostic_dict]}}}`.
- `_diagnostic_severity_name` — DiagnosticSeverity(severity).name (lsp_diagnostics.py:126);
- fallback name_path `<file>` (константа `LspApi.FILE_LEVEL_DIAGNOSTIC_BUCKET` lsp_api.py:309).

### 3.5. DiagnosticsContext + EditingToolWithDiagnostics (lsp_diagnostics.py:210-259, tools_base.py:433-457)
- `EditingToolWithDiagnostics.diagnostics_context(*paths)` — контекст-менеджер; snapshot ДО, diff ПОСЛЕ, `format_result` инжектит `diagnostics[warning-or-higher]` в JSON-результат (lsp_diagnostics.py:219).
- **КРИТИЧНО**: `ENABLE_DIAGNOSTICS_DEFAULT = False` (lsp_diagnostics.py:211, tools_base.py:439). Комментарий: per-edit diagnostics «questionable», т.к. отдельные правки намеренно вносят ошибки (несовпадение сигнатур), которые устраняются следующими правками.
- Все editing-tools оборачивают `apply` в `with self.diagnostics_context(...)` (symbol_tools.py:369-370, file_tools.py:78).

### 3.6. LS-сторона (solidlsp/ls.py:680-1055)
- `_store_published_diagnostics` (ls.py:680) — кэш publishDiagnostics по URI, `generation` (монотонный счётчик), `Condition.notify_all`;
- `_canonicalize_published_diagnostics_uri` (ls.py:727) — Windows-нормализация `file:///c%3A` ↔ `file:///C:/`;
- `request_text_document_diagnostics` (ls.py:978) — **pull-diagnostics** (`textDocument/diagnostic`, LSP 3.17) с fallback на **publish**; `_supports_pull_diagnostics` (ls.py:920) защищает серверы, падающие на неизвестных методах (Julia);
- `_diagnostic_matches_range` (ls.py:788) — нормализует инвертированные range (Julia), `end_line<0` = без верхней границы;
- `_diagnostic_matches_min_severity` (ls.py:804) — `severity <= min_severity` (1=Error..4=Hint).

---

## 4. LSP-менеджер: серверы, lifecycle, sync

Файл `src/serena/ls_manager.py` (385 LOC).

### 4.1. LanguageServerFactory (ls_manager.py:29-71)
Создаёт `SolidLanguageServer.create(ls_config, project_root, timeout, solidlsp_settings)`. Прокидывает workspace_folders, additional_workspace_folders, ignored_paths, encoding, ls_specific_settings.

### 4.2. LanguageServerManager (ls_manager.py:73-285)
- `from_languages(languages, factory, project)` (ls_manager.py:102) — **параллельный старт** в потоках `StartLSThread`; если хоть один сервер упал — fail-fast: остановить успешные и raise `LanguageServerManagerInitialisationError` (ls_manager.py:151-161). Обоснование: пользователь хочет работать со всеми языками.
- `_default_language_server` (ls_manager.py:90) — первый в iteration order;
- `_get_suitable_language_server(relative_path)` (ls_manager.py:172) — первый сервер, для которого `not is_ignored_path(path, ignore_unsupported_files=True)`;
- `get_language_server(relative_path)` (ls_manager.py:179) — при >1 серверах выбирает подходящий, иначе default; всегда `_ensure_functional_ls` (рестарт при `not is_running()`, ls_manager.py:163-170);
- `restart_language_server` (ls_manager.py:198), `add_language_server` (209), `remove_language_server` (219) — динамическое управление;
- `stop_all` (244), `save_all_caches` (255);
- `sync_file_system_changes` (ls_manager.py:273) → `_file_change_notifier.poll_and_notify()`.

### 4.3. LanguageServerFileChangeNotifier (ls_manager.py:287-385)
- `poll_and_notify()` (ls_manager.py:303): обходит `project.gather_source_files()`, строит `{rel_path: mtime}`, диффит с предыдущим снапшотом под локом, формирует события (Created/Changed/Deleted), шлёт `didChangeWatchedFiles` всем серверам.
- **Приём** (ls_manager.py:376-383): после `didChangeWatchedFiles(Created)` дополнительно `notify_files_created` + open/close цикл (`with ls.open_file(path): pass`) — pyright не сворачивает новый файл в cross-file graph без parse+bind.
- Вызывается ПЕРЕД symbolic-запросами (не по таймеру): см. lsp_api.py:545,583,637,662,697,741,764.

### 4.4. Список language servers: `LanguageServerId` enum (solidlsp/ls_config.py:120-353)
**77 значений**: csharp, python, rust, java, kotlin, typescript, go, ruby, dart, cpp, cpp_ccls, php, php_devsense, r, perl, clojure, elixir, elm, terraform, swift, bash, crystal, cue, zig, lua, lua_emmylua, luau, nix, erlang, ocaml, al, fsharp, rego, scala, julia, fortran, haskell, haxe, lean4, groovy, vue, svelte, astro, powershell, pascal, matlab, msl, bsl, ada, gdscript, qml, gleam, nextflow, wolfram + экспериментальные (typescript_vts, python_jedi/ty/pyrefly/basedpyright, csharp_omnisharp, ruby_solargraph, php_phpactor/phpantom, julia_fatou, markdown, latex, yaml, json, toml, hlsl, systemverilog, solidity, ansible, html, scss, angular, deno).

Категоризация (ls_config.py:364-420): `is_secondary`, `is_experimental`, `is_superset_language`, `is_programming_language`. `get_priority` (ls_config.py:423): experimental/secondary/non-programming = 0, superset = 1, regular = 2.
`iter_all(include_experimental, include_secondary, include_non_programming)` (ls_config.py:355) — фильтр для авто-детекта.
`get_source_fn_matcher` (ls_config.py:447, `@cache`) — расширения файлов на язык (python→.py,.pyi; typescript→.ts/.tsx/.js/.jsx/.mts/...).
`LanguageServerRegistry` (ls_config.py:1054) — singleton, обнаруживает внешние адаптеры через entry-points (ls_config.py:1087).

### 4.5. Выбор LS при авто-создании проекта (serena_config.py:369-420)
`_determine_project_language_servers`: `compute_language_server_support_composition` (util/inspection.py:38) — процент файлов на язык (знаменатель = файлы, распознанные хоть одним LS); сортировка по (%, priority); топ-язык включается, остальные — интерактивный запрос.

---

## 5. Contexts (16) + Modes (9) — полная таблица

### 5.1. Схема (context.template.yml, mode.template.yml)
Поля: `description, prompt (Jinja2), excluded_tools, included_optional_tools, fixed_tools, excluded_apis, included_apis, tool_description_overrides, single_project, structured_tool_output`.
- `fixed_tools` НЕ совместим с `excluded_tools`/`included_optional_tools` (serena_config.py:168-171);
- `single_project: true` + проект при старте → toolset минимизируется под project config, `activate_project` отключается (agent.py:836-858);
- `structured_tool_output: false` — для клиентов с багом распаковки (claude-code.yml:56).

### 5.2. Contexts (16 + template)
| Context | excluded_tools | prompt | single_project | overrides |
|---|---|---|---|---|
| `desktop-app` (DEFAULT) | [] | desktop chat, full toolset | false | {} |
| `agent` | initial_instructions | «agent context» | false | {} |
| `oaicompat-agent` | initial_instructions | OpenAI-compatible | false | {} |
| `chatgpt` | [] | desktop, короткие описания (лимит 30 tools) | false | find_symbol, replace_content, search_for_pattern |
| `ide` | create_text_file, read_file, execute_shell_command, find_file, list_dir | IDE, не читать целые файлы | true | {} |
| `antigravity` | (те же 5) | generic IDE | true | {} |
| `vscode` | (те же 5) | +activate/initial | true | {} |
| `jb-ai-assistant` | (те же 5) | IDE | true | {} |
| `jb-copilot-plugin` | create_text_file, read_file, execute_shell_command | +activate | true | {} |
| `junie` | 7 (вкл. replace_content, search_for_pattern) | IDE | true | {} |
| `codex` | create_text_file, read_file, execute_shell_command, replace_content, find_file, list_dir | Codex IDE | false | {} |
| `copilot-cli` | create_text_file, read_file, execute_shell_command, replace_content | CLI | true | {} |
| `claude-code` | 6 (вкл. search_for_pattern) | **жёсткий prompt**: FORBIDDEN read/edit, «tool-search» | true | {} + structured_tool_output:false |
| `codebuddy` | 6 (те же) | как claude-code | true | {} |
| `zcode` | 6 | как claude-code (Z.ai) | true | {} |
| `grok` | 6 | как claude-code (Grok Build) | true | {} |

**Ключевой паттерн prompt'ов CLI-агентов** (claude-code/codebuddy/zcode/grok): жёсткие правила «Read FORBIDDEN для discovery», таблица маппинга (get_symbols_overview / find_symbol / replace_symbol_body / rename_symbol), «Disallowed reasoning» — явный запрет рационализаций («I already know the path», «one Read быстрее трёх Serena»). Это **anti-preference-hijack** — урок для prime-agent.

`cc_system_prompt_override` (system_prompt.yml) — полная замена системного промпта для Claude Code с таблицей «Task → Tool».

### 5.3. Modes (9 + template)
| Mode | excluded_tools | excluded_apis | prompt суть |
|---|---|---|---|
| `editing` | replace_lines, insert_at_line, delete_lines | — | symbolic editing (rename/safe_delete/replace_symbol_body/insert_*); `{{ tool_names[...] }}`; **Jinja-условия** на available_tools |
| `planning` | 9 editing-инструментов | — | только анализ, не писать код |
| `one-shot` | [] | — | автономно, без вопросов |
| `benchmark` | memories+onboarding | mem | как one-shot + без памяти |
| `interactive` | [] | — | уточнять у пользователя |
| `onboarding` | 8 editing-инструментов | — | первый раз, собирать memories |
| `no-memories` | 6 memory + onboarding | mem | memories выключены |
| `no-onboarding` | onboarding | mem.onboarding | без онбординга, память есть |
| `query-projects` | [] (+included: list_queryable_projects, query_project) | included_apis: ext | внешние проекты |

Internal modes (не переопределяются пользователем): `jetbrains` (constants.py: INTERNAL_MODE_YAMLS_DIR) — исключает LSP-tools, включает 14 jet_brains_*.

### 5.4. Как применяется (agent.py:780-868)
Порядок наложения `ToolSet.default().apply(...)`:
1. OpenDashboard (если openable); 2. serena_config; 3. context; 4. base_modes (полностью); 5. dynamic modes (in single-project — полностью; иначе только их inclusions); 6. SingleProjectExclusions (activate_project, get_current_config) + project_config; 7. `without_editing_tools()` если project.read_only.
Метки `LEGACY_TOOL_NAME_MAPPING` применяются внутри `apply`.

---

## 6. Facades + s.info() + REPL

### 6.1. FacadeApi/Facade (repl/facade.py)
- **Принцип**: метод экспонируется ⟺ декорирован `@facade_method(...)` (facade.py:320-322, 396-415). Имена с trailing `_` (напр. `symbols_`, `to_dict_`) — публичны внутри Serena, но НЕ для REPL; leading `_` — приватны.
- `@facade_method(optional, beta, can_edit, niche, uses_project_server, corresponding_tool)` (facade.py:274) — метаданные, зеркалят tool-маркеры.
- `Facade.from_api(api, api_scope)` (facade.py:590) — обходит `inspect.getmembers(api, ismethod)`, оборачивает в `FacadeMethod`, проверяет enabled через `ApiScope.is_method_enabled`.
- `Facade.__getattr__` (facade.py:702) — доступ только к enabled-методам; `__setattr__` запрещён (read-only, facade.py:706).

### 6.2. ApiScope (facade.py:460-560)
Обрабатывает `excluded_apis`/`included_apis` из config/context/modes/project; `exclude_editing()` — read-only проект (facade.py:517). Логика `is_method_enabled` (facade.py:524): optional-метод или невключённый фасад → требует явного inclusion; иначе enabled, если не исключён.

### 6.3. ReferencedType — прогрессивная документация типов (facade.py:92-233)
- Типы, достижимые через аннотации, **обнаруживаются автоматически** (`_discover_referenced_types`, facade.py:601); явное объявление нужно лишь для курирования (`members`, `provide_info_with_facade`).
- `describe()` (facade.py:188) — рендер: docstring + attributes + methods (с сигнатурами) + capabilities (`__len__`→len(), `__iter__`→iteration, facade.py:111).
- `_CAPABILITIES` (facade.py:111-116) — перевод dunder-протоколов в человекочитаемые фичи.

### 6.4. SerenaRepl + s.info() (repl/repl.py)
- `SerenaReplEntrypoint` (repl.py:47) — фасады как атрибуты `s`; `s.info(*items)` (repl.py:183) — прогрессивное раскрытие: без аргументов → `overview()` (список фасадов+методов с return-типами, repl.py:168); `s.info("lsp")` → полный describe фасада; `s.info("lsp.find_symbol")` → метод; `s.info("LanguageServerSymbol")` → тип.
- `_describe_type` (repl.py:225) — транзитивные referenced types (BFS), **дедупликация в рамках сессии** через `session.described_type_names` («documented earlier in this session», repl.py:257) — экономия токенов.
- `SerenaRepl.execute` (repl.py:316) — код исполняется как ячейка notebook: statements + trailing expression (её значение = результат); `_represent` (repl.py:300) — Representable рендерит себя, list/tuple поэлементно построчно.
- `_run` (repl.py:336) — `ast.parse` → разделение trailing `Expr` → exec + eval с сохранением номеров строк.
- `_format_error` (repl.py:359) — локализует ошибку в строке кода, подсказка про notebook-семантику (не `return`).
- **Persistence**: `session.repl_namespace` (session.py:15-33) — top-level имена переживают вызовы; `s.vars()` (repl.py:127) / `s.clear()` (repl.py:144).

### 6.5. Representable/Renderer (repl/representable.py)
- `Representable.represent()` — объект рендерит себя для LLM; `RepresentableViaRenderer` + `Renderer[T]` (representable.py:15).
- `Renderer._limit_length` (representable.py:33) → `TextOutputUtils.limit_length` (text_utils.py:989).
- `JsonObject`/`JsonObjectRenderer` (representable.py:83-106).

### 6.6. Facades: 8 штук, 65 методов
`cfg` (2), `fs` (5), `edit` (8), `mem` (7), `shell` (1), `ext` (3), `lsp` (10), `jb` (14).
`create_default_facade_list` (agent.py:1325): cfg, fs, edit, mem, shell + ext (optional) + backend-specific (lsp/jb).

### 6.7. Renderers с прогрессивным сокращением (lsp_api.py)
- `LspSymbolCollectionRenderer` (lsp_api.py:118): при превышении лимита → mapping {file: [name_paths]} (lsp_api.py:164-165);
- `LspSymbolsOverviewRenderer` (lsp_api.py:189): → depth-0 → counts by kind;
- `LspReferenceCollectionRenderer` (lsp_api.py:236): → refs без контекста → counts per file → «Found N references»;
- группировщики (lsp_api.py:316-318): `find_symbol` children by kind, `references` by [relative_path, kind], `overview` by kind.

### 6.8. External projects (ext_api.py)
`ExternalProjectContextManager` — `with s.ext.project_context("name"):` временно активирует внешний проект (read-only или read-write), фасады переключаются; вложенность запрещена (ext_api.py:38). Для LSP — через ProjectServer (project_server.py).

---

## 7. prompt_factory + templates + system_prompt

### 7.1. PromptFactory (generated_prompt_factory.py, interprompt/)
- `interprompt` — автогенерация фабрики методов из Jinja2-шаблонов (prompt_factory.py:52); `JinjaTemplate` использует `SandboxedEnvironment` (jinja_template.py:22) и `jinja2.meta.find_undeclared_variables` для извлечения параметров.
- `SerenaPromptFactory` (prompt_factory.py): user-шаблоны (`~/.serena/prompt_templates`) приоритетнее встроенных (resources/config/prompt_templates).

### 7.2. Переменные шаблонов (agent.py:1033-1091, docs/050_configuration.md)
- `available_tools` — имена доступных tools (`{% if 'replace_content' in available_tools %}`);
- `available_markers` — имена ToolMarker-классов, у которых есть хотя бы один tool (`ToolMarkerSymbolicRead`);
- `tool_names` — mapping канонических имён → эффективных (`find_symbol`→`jet_brains_find_symbol` или → `lsp.find_symbol` в REPL); ПРЕДПОЧИТАТЬ `{{ tool_names['find_symbol'] }}` хардкоду;
- `embed_memory(name)` (agent.py:1107) — инлайн памяти в промпт (context/mode/project prompts, НЕ system_prompt).

### 7.3. Сборка system_prompt (agent.py:1165-1210)
`create_system_prompt`:
1. создаёт сессию (session_id);
2. глобальные memories (только имена, agent.py:1179);
3. `context_system_prompt` + `mode_system_prompts` (только не показанные ранее в сессии);
4. project activation message (если проект активен);
5. session id в `<session>` tag.
Всё обёрнуто в `<serena>` tag; context → `<context>`, mode → `<mode name="...">`, project → `<active-project>`.

### 7.4. Ключевые приёмы в system_prompt.yml
- «Line numbers returned by Serena's tools are 0-based!»;
- **Батчинг параллельных вызовов**: «Serena executes tool calls one at a time internally... you can safely issue them as parallel tool calls... maximizing parallel tool calls is the single biggest lever on cost»;
- прогрессивное чтение: сначала overview, потом body только нужных символов; «Once you have read a full file, there is no point re-analysing it»;
- `<memory>`-инлайн и «You have hereby read the Manual and do not need to read it again» (дедупликация).
- `connection_prompt`: «CRITICAL: call initial_instructions before coding» — bootstrap для клиентов без системного промпта.

### 7.5. MCP-адаптер (mcp.py)
- docstring apply-метода → MCP tool description; параметры парсятся `docstring_parser` (mcp.py:70-97);
- `tool_description_overrides` из контекста применяются здесь (mcp.py:74);
- `_sanitize_for_openai_tools` (mcp.py:173) — integer→number, удаление null из union (для Codex);
- `ToolAnnotations(read_only_hint=not can_edit, destructive_hint=can_edit)` (mcp.py:112-115).

---

## 8. project.py + memories

### 8.1. Project (project.py:34-784)
- Конструктор (project.py:35): создаёт `.serena/`, MemoryManager, асинхронно собирает ignore-spec в фоновом потоке (`_gather_ignorespec`, project.py:87) — быстрая активация;
- ignore-spec: global `[".git"] + ignored_paths` + project ignored_paths + gitignore (project.py:87-130); explicit config имеет final say;
- `create_language_server_manager` (project.py:641): LS timeout = tool_timeout - 5;
- `ls_sync_file_system_changes` (project.py:765), `shutdown` (project.py:773);
- `gather_source_files`, `gather_project_files`, `scan_directory`, `search_project_files_for_pattern`.

### 8.2. Memories (memories/)
- Файлы `.md`; project `.serena/memories/`, global `~/.serena/memories/global/` (memory_manager.py:29-31);
- `mem:`-префикс ссылок в backticks (`MEMORY_REF_PREFIX = "mem:"`, memory_reference_analysis.py:33);
- `MemoryManager` (memory_manager.py:28): `_sanitize_name` (правит ошибки LLM: mem:-префикс, .md, сепараторы, memory_manager.py:77); topics через `/`; `read_only_memory_patterns`, `ignored_memory_patterns` (regex);
- `rename_references_to_memory` (memory_manager.py:126) — при переименовании правит все `mem:OLD`→`mem:NEW` (кроме read-only);
- `memory_reference_analysis.py`: referential integrity (`serena memories check`), fuzzy-поиск кандидатов (`compute_name_similarity`, line 104), autofix bare-ссылок;
- `memory_maintenance.md` (resources/) — конвенции: dense agent notes, `mem:`-ссылки, порог add/update.
- **Вывод**: система памяти дублирует continual harness prime-agent → НЕ переносим; но паттерн «имя + mem:-ссылки + integrity-check» полезен как модель для pa-graph.

---

## 9. НОВЫЕ находки (чего НЕТ в известном)

1. **`symbol_info_budget`** (symbol.py:627-635, serena_config.py:969, template yml:216) — time-budget для hover-запросов (default 10s; мотивация в шаблоне: clangd имеет медленный request_hover); при превышении оставшиеся символы получают `info=None` (partial results). Группировка по файлу + per-file cache (line,col).
2. **EOF-конвенция body** (ls.py:257-268) — range на `(len(lines),0)` = «до EOF»; иначе InvalidTextLocationError (защита от garbage body).
3. **SymbolBody ~40 байт** (ls.py:242) — 4 int + ref на shared lines-buffer; SymbolBodyFactory разделяет буфер.
4. **Имя символа НЕ должно содержать `/`** (ls.py:1958-1962) — иначе name_path неразрешим (Erlang name/arity); язык должен подставлять другой символ.
5. **Overload_idx добавляется LS-слоем** (ls.py:2087-2094) — при дублировании имён среди siblings.
6. **Cross-file reference wait** (ls.py:1057, `_get_wait_time_for_cross_file_referencing` default 2s) — для LS без сигнала «finished initializing».
7. **Python-hack для variable-references** (ls.py:2490-2510) — TODO/HORRIBLE HACK: поиск containing symbol для `instance.status = ...` эвристикой по тексту (специфично для Python).
8. **Import-vs-definition discrimination** (ls.py:2545-2560) — эвристика по selectionRange для отсечения импортов (LSP не различает).
9. **`niche`-методы** (facade.py:248) — редко нужные методы в описании фасада даются только первой строкой + ссылка на `s.info(...)` — экономия токенов.
10. **`_CAPABILITIES`** (facade.py:111) — dunder→человекочитаемое (len/iteration/indexing/with).
11. **Occurrence-id для replace_in_files** (text_utils.py:571) — `path:index@sha256(content:start:end)[:16]`, стабильный content-anchored id; re-валидация при применении (text_utils.py:922).
12. **Ambiguity-детекция regex** (text_utils.py:439-451) — если pattern матчит внутри уже сматченного текста (multiline) → ValueError «ambiguous»; заставляет LLM уточнить паттерн.
13. **Backreference синтаксис `$!1`** (text_utils.py:463) — вместо `\1` (обход escaping).
14. **Hooks anti-grep/anti-read** (hooks.py:141-643) — счётчики consecutive grep/read (порог 3/3/4), deny с напоминанием о symbolic tools; rate-limit `_MIN_DENY_INTERVAL_SECONDS=120`. Интеграция с Codex/Claude Code.
15. **`SessionRegistry`** (session.py:35) — LRU + idle-TTL эвикция сессий (max 100, 48h), session_id от LLM.
16. **`TaskExecutor`** (task_executor.py:20) — сериализация tool-вызовов через очередь + timeout; параллельные tool-calls агента выполняются последовательно (важно: нет race).
17. **`_MIN_...` reset-периоды hook'ов** (hooks.py:175-186) — период гейтит gap между СОСЕДНИМИ вызовами, не absolute window.
18. **`path_to_serena_data_folder` + fallback** (docs): `.serena` в проекте или `$projectDir/.serena` / `/metadata/$projectFolderName/.serena`.
19. **`trusted_project_path_patterns`** (serena_config.py:943) — untrusted проекты: игнор `activation_command`, `ls_specific_settings`, symlink-escape.
20. **Line-ending нормализация** (project.py: line_ending lf/crlf/native; docs: `git config core.autocrlf true`).
21. **`get_apply_docstring_from_cls`** (tools_base.py:196) — статические метаданные из `cls.__dict__` (для генерации MCP-tools в отдельном процессе без сериализации).
22. **`_wrapped_tool_response`** (tools_base.py:427) — обёртка `{message, response}` для делегирующих tools.
23. **`text_document_diagnostic` pull + publish fallback** (ls.py:978-1055) — комбинированный механизм.
24. **`LanguageServerRegistry`/`LanguageBackendRegistry`** (ls_config.py:1054, language_backend.py:171) — entry-points discovery (внешние адаптеры).

---

## 10. ТОП-10 для переноса (pa-lsp / pa-graph / edit) + опасности

| # | Приём Serena | Куда в prime-agent | Ценность | Опасность/нюанс |
|---|---|---|---|---|
| 1 | **name_path-адресация** (relative `/abs`, `[overload]`, substring) — symbol.py:145-219 | pa-lsp symbols.rs | ★★★★★ | Имя символа не должно содержать `/`; overload_idx нужно заполнять на LS-слое |
| 2 | **diagnostics-к-символу** (`file→severity→name_path`) — lsp_diagnostics.py:95-149 | pa-lsp ledger.rs | ★★★★★ | Требует `find_diagnostic_owner_symbol` (symbol_at_location + подъём от low-level) |
| 3 | **new-vs-existing diff** (`DiagnosticIdentity` + snapshot) — lsp_diagnostics.py:24-208 | pa-lsp ledger.rs | ★★★★★ | Snapshot ДО правки обязателен; generation-механизм; НЕ навязывать (Serena держит off) |
| 4 | **SymbolBody ~40 байт + EOF-конвенция** — ls.py:232-311 | pa-lsp types.rs | ★★★★☆ | Out-of-range range → ошибка, не угадывать |
| 5 | **symbol_info_budget** (hover time-budget + batch по файлам) — symbol.py:634 | pa-lsp retriever | ★★★★☆ | Partial results (None) при превышении; 0 = отключить |
| 6 | **FS-change notifier + open/close для Created** — ls_manager.py:287-385 | pa-lsp manager.rs | ★★★★☆ | Вызывать перед symbolic-запросами; pyright нужен open/close |
| 7 | **replace_in_files: dry_run + occurrence-id + ambiguity-детекция** — text_utils.py:519-660 | pa-edit | ★★★★☆ | occurrence-id = `path:idx@sha256(content:start:end)[:16]`; re-валидация при apply |
| 8 | **Прогрессивное сокращение результатов** (overview→depth0→counts; refs→counts) — lsp_api.py:118-280 | pa-lsp renderers | ★★★★☆ | max_answer_chars (default 150k); каскад closures |
| 9 | **Facades + s.info() прогрессивная документация** — facade.py, repl.py:186 | pa-tools API | ★★★☆☆ | Много мета-магии (inspect); в Rust — кодоген/ручной реестр |
| 10 | **Tool-маркеры + auto-registry + single-project toolset** — tools_base.py:71-105,477; agent.py:780 | pa-tools | ★★★☆☆ | Маркеры (SymbolicRead/CanEdit/Optional/Beta) → фильтрация toolset; tombstone-список |

### Опасности (что НЕ копировать слепо)
1. **Не навязывать диагностику после каждой правки** — Serena держит `ENABLE_DIAGNOSTICS_DEFAULT=False` (lsp_diagnostics.py:211): правки часто намеренно ломают (сигнатуры), чинится следующими.
2. **Memory-система НЕ нужна** — дублирует continual harness prime-agent.
3. **JetBrains-бэкенд НЕ нужен** — только LSP (13 tools + 14 facade-методов).
4. **Python-hack для variable-references** (ls.py:2490) — не портировать; решить иначе.
5. **Pull-diagnostics падает на некоторых LS** (Julia) — нужен `_supports_pull_diagnostics` guard.
6. **`_normalize_symbol_name`** должен убирать parameter-lists и запрещать `/`.
7. **Параллельные tool-calls агента** Serena сериализует (TaskExecutor) — при батчинге нет race; если в prime-agent параллельно — нужны локи на LS-клиент/буферы.
8. **single_project toolset** экономит контекст, но ломает `activate_project` — осознанный trade-off.
9. **occurrence-id re-валидация** обязательна (контент мог измениться — text_utils.py:922).
10. **`symbol_info_budget` по умолчанию 10s** — на больших C++/Clangd может давать partial results.

### Архитектурный синтез для Rust
- `pa-lsp`: `symbols.rs` (NamePathMatcher + Symbol + body), `retriever.rs` (find/references/implementations/declaration + info-batch), `manager.rs` (parallel start, restart, sync, LS-выбор), `ledger.rs` (DiagnosticIdentity + snapshot + diff + grouped), `renderers.rs` (progressive shortening).
- `pa-graph`: name_path как узел графа; `mem:`-подобные ссылки (integrity-check) для связей символов/модулей.
- `pa-edit`: replace_in_files (dry_run, occurrence-id, ambiguity), symbol body replace/insert/delete (EOF-конвенция, empty-line heuristics code_editor.py:146-215,278-297).
- Tools: маркеры (CanEdit/SymbolicRead/SymbolicEdit/Optional/Beta) → фильтрация toolset; auto-registry через рефлексию Rust (enum/derive).
