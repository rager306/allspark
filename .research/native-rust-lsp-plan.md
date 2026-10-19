# Нативный Rust-LSP в prime-agent (БЕЗ MCP, БЕЗ Python)

## 0. Опровержение прежнего вывода
Прежний документ предлагал Serena-MCP. ОТБРОШЕНО. Ниже — нативный Rust-путь.

## 1. Нативные Rust-инструменты — ПЕРВЫЙ КЛАСС (проверено)
- `AgentTool` — полноценный Rust-трейт (`crates/pa-agent/src/types.rs:498`):
  name/description/parameters/label/prepare_arguments/accepts_alias/execute/execution_mode.
- `AgentContext.tools: Vec<Arc<dyn AgentTool>>` (`types.rs:535`) идёт ПРЯМО в wire-запрос
  провайдеру (`crates/pa-agent/src/agent_loop/response.rs:128`):
  ```rust
  tools: context.tools.iter().map(|tool| ToolDefinition {
      name: tool.name().to_string(),
      description: tool.description().to_string(),
      parameters: tool.parameters().clone(),
  }).collect(),
  ```
- Имена активных tools идут в system prompt (`engine.rs:459,521` → `selected_tools`).
- Диспетчер вызова: `agent_loop/tools.rs::find_tool` + `execute_tool_calls` (parallel/sequential).

**Вывод: нативный Rust-tool = штатный путь. Upstream просто наполняет `config.tools = vec![]`
и всё делает через python_repl. Нам ничто не мешает положить в `config.tools` свои.**

## 2. LSP на Rust — это НЕ порт 200KB TS
Ключевое открытие: `async-lsp` — **готовый Rust-фреймворк для LSP-КЛИЕНТА** (не только сервера).
Уже в cargo-кэше: `async-lsp-0.2.3`, `lsp-types-0.95.1`.

`async-lsp` даёт:
- `MainLoop::new_client(|server| ...)` — клиентский main loop
- `Router` + `LanguageServer` trait: `initialize`, `did_open`, `hover`, `references`,
  `rename`, `definition`, `symbols`, `shutdown`, `exit` — все LSP-методы типизированы
- транспорт: `run_buffered(stdout, stdin)` поверх pipes
- middleware: `ConcurrencyLayer`, `CatchUnwindLayer`, `TracingLayer`
- `client_monitor::ClientProcessMonitor` — мониторинг процесса сервера

`lsp-types` даёт все типы: `HoverParams`, `GotoDefinitionParams`, `ReferenceParams`,
`RenameParams`, `DocumentSymbolParams`, `PublishDiagnosticsParams`, `TextDocumentItem`, `Url`.

Пример `async-lsp/examples/client_builder.rs` (~150 строк) делает ровно то, что нужно:
спавн `rust-analyzer` → `initialize` → `did_open` → `hover` → `shutdown`. Это шаблон.

## 3. Зависимости — ВСЁ УЖЕ В КЭШЕ
| Crate | Версия | Статус |
|---|---|---|
| async-lsp | 0.2.3 | cached |
| lsp-types | 0.95.1 | cached |
| tower | 0.4.13 | cached |
| tower-layer | 0.3.3 | cached |
| tower-service | 0.3.3 | cached |
| futures | 0.3.32 | cached |
| pin-project-lite | 0.2.17 | cached |
| async-process | 2.5.0 | cached |
| tokio (process) | 1 (full) | уже в workspace |
| serde_json | 1 (preserve_order) | уже в workspace |

LSP-серверы уже установлены: `rust-analyzer`, `typescript-language-server`, `gopls`.
omp `defaults.json` содержит 57 серверов с `command/args/fileTypes/rootMarkers` — берём таблицу.

## 4. Архитектура (нативный Rust, без MCP/Python)

```
┌──────────────────────────────────────────────────────────────┐
│ МОДЕЛЬ: python_repl + НОВЫЙ нативный tool `lsp`              │
│  (оба в config.tools → wire-запрос, response.rs:128)         │
└───────────────────────────┬──────────────────────────────────┘
                            │ tool_call {name:"lsp", args:{action,...}}
                            ▼
┌──────────────────────────────────────────────────────────────┐
│ crates/pa-lsp  (НОВЫЙ крейт, ~1.5-2K LOC)                    │
│                                                               │
│  LspTool: impl AgentTool                                      │
│    name() = "lsp"; parameters() = JSON-schema {action,...}    │
│    execute() -> dispatch по action                            │
│                                                               │
│  LspManager (per-session, Arc)                                │
│    clients: HashMap<(server,cwd), LspClientHandle>            │
│    ensure_client(file) -> spawn сервер по расширению          │
│                                                               │
│  LspClientHandle (async-lsp MainLoop)                         │
│    initialize/did_open/did_change/did_save                    │
│    hover/definition/references/rename/symbols/diagnostics     │
│    PublishDiagnostics -> diagnostics-ledger (DashMap)         │
│                                                               │
│  servers.rs: таблица из omp defaults.json (57 серверов)       │
└───────────────────────────┬──────────────────────────────────┘
                            │ spawn + JSON-RPC over stdio
                            ▼
              rust-analyzer / tsc / gopls / clangd / ...
```

### actions (паритет с omp tool.ts)
`status | diagnostics | definition | type_definition | implementation | references |
hover | symbols | rename | code_actions | capabilities | request`

## 5. Точки интеграции в prime (минимальные)

| # | Место | Изменение |
|---|---|---|
| 1 | новый `crates/pa-lsp` | крейт: LspTool + LspManager + servers.rs |
| 2 | `Cargo.toml` workspace | members += pa-lsp; deps += async-lsp, lsp-types, tower, futures, async-process |
| 3 | `crates/pa-core/src/session_engine/engine.rs` | в `create_session`: если LSP включён — push `Arc::new(LspTool::new(...))` в `tools` |
| 4 | `crates/pa-core/src/prompts/layers/core.md` | секция про `lsp` tool (или динамический tail) |
| 5 | settings | `lsp: { enabled, servers: {...}, idleTimeoutMs }` |
| 6 | daemon lifecycle | прокинуть `LspManager` (per-worker, как provisioner) |

Пункт 3 — единственное реальное вторжение в ядро, и оно аналог существующего
ipython-push (engine.rs:446-455). Seam для settings — как `prewarm_ipython_kernel`.

## 6. Почему это рентабельно (в отличие от Python/MCP)

| Критерий | Python/MCP (отброшено) | Нативный Rust |
|---|---|---|
| Процесс на запрос | Python-интерпретатор + MCP-фреймворк | нет (in-process) |
| Сериализация | JSON через fd → Python → JSON | JSON-RPC напрямую в LSP-сервер |
| Транспорт | MCP-протокол поверх LSP поверх stdio | LSP поверх stdio |
| Зависимости | mcp SDK, httpx, anyio, ... | async-lsp + lsp-types (cached) |
| Паритет с upstream | внешний костыль | нативный tool, как ipython |
| Бинарь | +внешние процессы | +~200KB кода, +LSP-серверы (уже стоят) |
| Диагностика | через Python-исключения | типизированный LSP |
| Скорость | 2 IPC-хопа | 1 IPC-хоп |

## 7. Открытые вопросы (требуют решения ДО кода)
1. **Жизненный цикл клиента**: per-session или per-worker? omp использует shared mux-daemon.
   Для prime проще per-session `LspManager` в `SessionEngine`.
2. **Идле-таймаут**: omp останавливает клиентов по idle. Нужно ли? (экономия памяти)
3. **Синхронизация документов**: LSP требует did_open/did_change. Редактирование идёт
   через `edit` (Python-skill). Значит нужен hook: после успешного `edit` → notify LSP
   (did_change) → обновить диагностику. Вариант: watcher на FS, либо host_request из edit.
4. **Интеграция с `edit`**: диагностика ПОСЛЕ правки — killer-feature Serena. Механизм:
   `edit` (Python) → host_request "lsp.did_change" → LspManager → diagnostics ledger.
5. **Что делать, если сервер не установлен**: `status` action + честная ошибка (как omp).
6. **Диагностики в промпте/ответе**: push (PublishDiagnostics) в ledger, читать при след. turn.
7. **Объём**: MVP = diagnostics + definition + references + symbols + hover + rename
   (6 actions). Остальное — позже.


## 8. Техническая проверка (ВСЁ ПОДТВЕРЖДЕНО)

### async-lsp 0.2.3 API (проверено в исходниках)
- `MainLoop::new_client(|server_socket| service) -> (MainLoop, ServerSocket)` — lib.rs:497
- `mainloop.run_buffered(input: impl AsyncRead, output: impl AsyncWrite)` — lib.rs:521
- `run_buffered` = `run(BufReader::new(input), output)` — подходит для `tokio::process::ChildStdout/ChildStdin`
- `Router::request::<R: Request,_>` / `Router::notification::<N: Notification,_>` — router.rs:85,117
- `LanguageServer` trait: `initialize`, `initialized`, `did_open`, `did_change`, `did_save`,
  `hover`, `definition`, `references`, `rename`, `document_symbol`, `shutdown`, `exit`
- middleware: `ConcurrencyLayer`, `CatchUnwindLayer`, `TracingLayer`
- `client_monitor::ClientProcessMonitor` (feature `client-monitor`) — следит за процессом сервера

### Версии совпадают с workspace
| dep | async-lsp требует | в кэше/lock | OK |
|---|---|---|---|
| lsp-types | 0.95.0 | 0.95.1 | ✅ |
| serde_json | 1.0.95 | 1 (workspace) | ✅ |
| serde | 1.0.159 | 1 (workspace) | ✅ |
| thiserror | 2 | 2.0.20 (lock) | ✅ |
| tower-layer | 0.3.2 | 0.3.3 | ✅ |
| tower-service | 0.3.2 | 0.3.3 | ✅ |
| futures | 0.3.28 | 0.3.32 | ✅ |
| pin-project-lite | 0.2.9 | 0.2.17 | ✅ |
| tokio | 1.27 (optional) | 1 full (workspace) | ✅ |
| rustix | 1 (optional) | 1.1.4 (lock) | ✅ |
| waitpid-any | 0.3 (optional) | 0.3.0 | ✅ |

### LSP-серверы в системе
- `rust-analyzer`: в toolchain 1.99.0 (`/root/.rustup/toolchains/1.99.0-.../bin/rust-analyzer`)
  (симлинк `~/.cargo/bin/rust-analyzer → rustup` работает через rustup proxy)
- `typescript-language-server` 6.0.0
- `gopls` v0.16.2

### Диск
60 GB свободно (нужен резерв 15 GiB для sibling). PoC-сборка допустима.

## 9. План PoC (доказательство жизнеспособности)

Цель: доказать, что нативный Rust-LSP работает в prime без Python/MCP.

### Шаг 1: минимальный крейт `crates/pa-lsp` (~400 LOC)
```
src/
  lib.rs        — публичный API: LspTool, LspManager
  tool.rs       — impl AgentTool: name="lsp", dispatch(action)
  client.rs     — LspClient: async-lsp MainLoop + spawn сервера
  servers.rs    — таблица серверов (Rust-only пока: rust-analyzer, tsc, gopls)
  types.rs      — action enum + request/response JSON
```

### Шаг 2: MVP actions (3 из 11)
`status | diagnostics | symbols` — достаточно для доказательства.

### Шаг 3: интеграция
- `create_session`: push `Arc::new(LspTool::new(cwd))` в `tools` (аналог ipython-push)
- prompt: секция в `core.md`
- проверить: модель вызывает `lsp` tool, получает диагностику rust-analyzer

### Шаг 4: проверка
- запустить allspark в /root/prime-agent
- tool call: `lsp(action="diagnostics", file="crates/pa-core/src/lib.rs")`
- ожидание: реальные diagnostics от rust-analyzer

### Оценка
| Метрика | Значение |
|---|---|
| Новый код (PoC) | ~400-600 LOC Rust |
| Новый код (полный MVP) | ~1500-2000 LOC Rust |
| Новые зависимости | async-lsp, lsp-types, tower-layer/service, futures, pin-project-lite (+опц. rustix, waitpid-any) |
| Вторжение в ядро | 1 место (create_session push) + prompt + settings |
| Риск для upstream merge | средний (новый крейт + 1 строка в engine.rs) |
| Митигация | FORK-ONLY маркеры, фича-флаг `lsp.enabled` (по умолчанию off) |

## 10. Развилки, требующие решения пользователя
1. **Делать PoC сейчас?** (сборка ~10-20 мин, диск есть)
2. **Scope MVP**: 3 actions (status/diagnostics/symbols) или сразу 6 (+definition/references/hover)?
3. **Синхронизация с `edit`**: как LSP узнаёт о правках из Python-skill `edit`?
   - (a) FS-watcher (notify crate) — автономно, но +зависимость
   - (b) host_request "lsp.did_change" из edit-skill — явно, но правка Python-skill
   - (c) did_open/did_change по запросу tool-а (ленивая синхронизация)
4. **Изоляция**: фича-флаг в settings (default off) — рекомендую.
