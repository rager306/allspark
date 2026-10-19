# Архитектурное моделирование: Serena-MCP / oh-my-pi-LSP в prime-agent

## 0. Как устроен tool surface prime-agent (ПРОВЕРЕНО ПО КОДУ)

### Факт №1: у модели РОВНО ОДИН tool — `python_repl`
`crates/pa-core/src/prompts/layers/core.md:3`:
> "The prime-agent harness has one tool: `python_repl`, a persistent CPython REPL.
> All other tools are 'programmatic tools', functions inside the Python REPL
> called via 'programmatic tool calling' (PTC)."

Подтверждено кодом:
- `crates/pa-cli/src/print_runtime.rs:1400` — `fn builtin_tools(_cwd) -> Vec<...> { Vec::new() }`
- `crates/pa-daemon/src/agent_engine/lifecycle.rs:1022` — `tools: vec![]`
- `crates/pa-core/src/session_engine/engine.rs:446` — `let mut tools = config.tools.clone();` затем добавляется ТОЛЬКО ipython
- `create_bash_tool_definition` / `create_edit_tool_definition` вызываются ТОЛЬКО из `golden_replay.rs` (тесты), не из production.

### Факт №2: `bash` и `edit` — это НЕ Rust-tools
- `bash` = `prime-agent-runtime/src/rlm/bash.py` (`BashHandle`, process groups), инжектится в namespace через `rlm_bootstrap.rs` (`bash = _prime_agent_rlm_module.bash`).
- `edit` = Python SKILL `skills/edit/src/edit/__init__.py` (~60 строк: `Path.read_text` + `str.count` + `write_text`).

### Факт №3: три точки расширения
```
A) Python skill:  skills/<name>/{SKILL.md, pyproject.toml, src/<mod>/__init__.py}
   → ставится uv-ом в kernel-venv, биндится в namespace (bootstrap)
   → примеры: edit, websearch, refine, compact, goal, agent-message

B) MCP server:    settings.json → mcpServers: {type: stdio|http, command, args, url}
   → клиент ЖИВЁТ В PYTHON (prime-agent-runtime/src/rlm/mcp.py)
   → доступ через `await mcp.call_tool(server, tool, args)` в REPL
   → примеры: gitnexus, codebase-memory-mcp, jina, exa

C) host_request:  Rust HostRequestHandlers::register("kind", handler)
   → seam: SessionEngineConfig.extra_host_handlers (engine.rs:42)
   → daemon подключает: lifecycle.rs:802 fn extra_host_handlers()
   → примеры: rlm.run, rlm.rename, rlm.collect, mcp.*, skill.*
```

### Факт №4: LSP/tree-sitter в prime-agent ОТСУТСТВУЕТ
`rg 'tree-sitter|tower-lsp|lsp-types' Cargo.toml crates/*/Cargo.toml` → только в скопированных `pa-edit-src-*` (untracked). В основном workspace — ноль.


## 1. Что даёт Serena и что даёт oh-my-pi LSP

### Serena (~70K LOC Python, MCP-сервер)
78 language servers через LSP; tools (из `/tmp/serena/src/serena/tools/`):
- `symbol_tools.py`: `find_symbol`, `find_referencing_symbols`, `replace_symbol_body`,
  `insert_after_symbol`, `insert_before_symbol`, `rename_symbol`, `get_symbols_overview`
- `file_tools.py`: read/write/search файлов
- `query_project_tools.py`, `memory_tools.py`, `workflow_tools.py`, `cmd_tools.py`
- `EditingToolWithDiagnostics` (tools_base.py:433) — правки + диагностика

### oh-my-pi LSP (~Rust? нет — TypeScript, `packages/coding-agent/src/lsp/`)
Один tool `lsp` с `action`:
`diagnostics | definition | type_definition | implementation | references | hover |
symbols | rename | code_actions | reload | status | capabilities | request | rename_file`
Плюс: mux daemon (переиспользование серверов), diagnostics-ledger, workspace-diagnostics,
deferred-diagnostics, writethrough (правки через LSP).

## 2. Три варианта интеграции (модель)

### Вариант A: Serena как MCP-сервер (НОЛЬ кода в prime-agent)
```
settings.json:
  mcpServers.serena = {
    type: "stdio",
    command: "uvx",
    args: ["--from","git+https://github.com/oraios/serena","serena","start-mcp-server",
           "--context","codex","--project","/root/prime-agent"]
  }
```
Модель: `await mcp.call_tool("serena","find_symbol",{...})`.
+ Ноль Rust-кода, ноль конфликтов с upstream, работает уже сейчас
+ 78 LSP-серверов «из коробки», symbol-level rename/references
− Тяжёлый Python-процесс (~70K LOC + LSP-серверы) в kernel-venv окружении
− Отдельный процесс на сессию; время старта; uvx каждый раз качает
− Не интегрировано в `edit`/prompt (модель должна знать про него из MCP-guidance)

### Вариант B: pi-edit crates как Rust-хост + новый host_request (ВСТРОЕНО)
```
crates/pa-ast   (pi-ast,   5 293 LOC, tree-sitter + 17 грамматик)
crates/pa-edit  (pi-edit, 23 844 LOC, hashline/replace/patch/apply_patch/sloppy)
crates/pa-diff  (pi-diff,  1 020 LOC, 0 deps)
crates/pa-walker(pi-walker,7 757 LOC)
→ host_request kind "edit.*"/"ast.*" в extra_host_handlers
→ тонкий Python-skill-фасад (skills/edit-ast/) вызывает host_request
```
+ Нативно, без внешних процессов; tree-sitter в процессе
+ Режимы hashline/patch устойчивее текущего «exact-unique-match»
− ~51K LOC нового Rust-кода (вариант B: без pi-vfs 6 408 LOC)
− Тянет tree-sitter + 17 грамматик (размер бинаря), 263 теста
− Прямой конфликт с upstream (новые крейты) — но изолируемо в fork

### Вариант C: гибрид (Serena-MCP сейчас + LSP-lite в Rust позже)
- Сейчас: Serena через MCP (нулевой риск).
- Позже, если понадобится скорость/нативность: подмножество LSP-клиента
  (только `diagnostics|definition|references|symbols|rename`) на Rust,
  как host_request. Не тащить все 78 серверов и pi-vfs.

## 3. Оценка «как встанет в prime» (по точкам расширения)

| Критерий | A: Serena MCP | B: pi-edit Rust | C: гибрид |
|---|---|---|---|
| Конфликт с upstream | нет | высокий (новые крейты) | низкий |
| Объём нового кода | 0 | ~51K LOC | 0 → ~8K LOC |
| Изоляция в fork | естественная | нужны FORK-ONLY маркеры | смешанная |
| Prompt-интеграция | через mcp-guidance | нативно в core.md | поэтапно |
| Вес бинаря | 0 (внешний) | +tree-sitter+17 грамматик | +только нужное |
| Риск для сессии | процесс может упасть | паника в процессе | низкий |
| Готовность | СЕЙЧАС | недели работы | дни→недели |

## 4. Ключевые архитектурные ограничения (найденные)

1. **MCP-клиент в Python, не в Rust.** Серверы запускает kernel (mcp.py), не daemon.
   Значит Serena-MCP живёт в kernel-venv окружении и виден только через REPL.
2. **`extra_host_handlers` — правильный seam для Rust-функций** (engine.rs:42 →
   lifecycle.rs:802). Новый `ast.*`/`lsp.*` handler регистрируется без правок ядра.
3. **Python-skill — правильный seam для фасада** (`skills/<n>/` + bootstrap binding).
   Rust-функция + Python-фасад = нативный инструмент без правок agent loop.
4. **Prompt знает только про programmatic tools.** Новый инструмент должен попасть
   в `core.md` (или динамический tail скиллов), иначе модель о нём не узнает.
5. **`python_repl` — единственный native tool.** Любое расширение = либо MCP,
   либо host_request + Python-фасад, либо новый skill.


## 5. КРИТИЧЕСКОЕ уточнение (проверено)

### pi-edit crates НЕ содержат LSP
- `pi-ast` (5 293 LOC): tree-sitter + ast-grep-core, 17 грамматик. **Нет LSP.**
- `pi-edit` (23 844 LOC): режимы hashline/replace/patch/apply_patch/sloppy. **Нет LSP.**
- `pi-diff` (1 020 LOC), `pi-walker` (7 083 LOC): утилиты. **Нет LSP.**
- `rg 'lsp-types|tower-lsp|LspClient' crates/pa-edit-src-*` → только ложные совпадения
  (`flush_ln`, `ln_batch_id`).

### oh-my-pi LSP — ЧИСТЫЙ TypeScript (~200 KB)
`/tmp/omp-src/packages/coding-agent/src/lsp/`:
- `client.ts` 63.9 KB, `tool.ts` 53.1 KB, `diagnostics.ts` 21.6 KB, `config.ts` 17.8 KB,
  `edits.ts` 16.6 KB, `servers.ts` 10.5 KB, `defaults.json` 12.7 KB
- `rg 'lsp-types|tower-lsp' /tmp/omp-src/crates` → ПУСТО

**Вывод:** «взять LSP-часть oh-my-pi» = портировать ~200 KB TS на Rust.
Копировать нечего — в Rust-крейтах omp LSP нет.

### Значит pi-edit даёт: AST-правки (tree-sitter), НЕ LSP-семантику
pi-edit умеет: hashline-патчи, AST-guided replace, diff — но НЕ:
`find_referencing_symbols`, `rename_symbol` (semantic), `diagnostics` от компилятора,
`definition`, `hover`. Для семантики нужен LSP-сервер (Serena/собственный клиент).

## 6. ФИНАЛЬНЫЙ ВЕРДИКТ

### Что реально можно взять «как есть»
| Источник | Что | Язык | Готовность |
|---|---|---|---|
| **Serena** | symbol tools + 78 LSP-серверов | Python/MCP | **сейчас, через mcpServers** |
| pi-edit crates | AST-правки (tree-sitter) | Rust | копируемо, но ~51K LOC |
| oh-my-pi LSP | LSP-клиент/диагностика | **TS** | только порт, ~200 KB |

### Рекомендация (порядок)
1. **Немедленно (0 кода):** Serena через `settings.json → mcpServers` (Вариант A).
   Модель получает `find_symbol`/`rename_symbol`/`references`/`diagnostics` через
   `await mcp.call_tool("serena", ...)`. Ноль конфликтов с upstream.
2. **Средний срок:** если нужны нативные AST-правки — подмножество pi-edit
   (`pa-ast` + `pa-diff`, БЕЗ pi-walker/pi-vfs) как Rust host_request
   + тонкий Python-skill-фасад. ~6.3K LOC вместо 51K.
3. **Долгий срок / не рекомендую:** порт LSP-клиента omp (200 KB TS → Rust).
   Оправдано только если Serena-MCP окажется слишком медленным/тяжёлым.

### Архитектурная развилка
- Хотим **семантику символов** (rename/references/diagnostics) → LSP, только Serena
  (или собственный LSP-клиент, но это большой порт).
- Хотим **устойчивые структурные правки** (без LSP) → pi-ast/pi-edit в Rust.
- Оба — ортогональны и могут сосуществовать: Serena-MCP для семантики,
  pa-ast host_request для правок.
