# DEBUGGER в oh-my-pi: разбор и переносимость в allspark

Источник: `/tmp/omp-src` (oh-my-pi / `omp`, workspace version `18.10.4` — `Cargo.toml:28`).
Режим: только чтение, сборки не запускались. Все ссылки `файл:строка` — на код omp,
если не помечено «(наш)».

---

## 0. Главный вывод в трёх строках

1. **«Debugger» в omp — это НЕ отладчик агента.** Это **DAP-клиент** (Debug Adapter
   Protocol), которым агент отлаживает *пользовательскую программу* (gdb/lldb-dlv/
   debugpy/js-debug). Второй, отдельный слой — `/debug` / `Shift+Ctrl+D` — это
   **диагностический инспектор самого omp** (логи, raw SSE провайдера, профили,
   системная инфа, бандлы отчётов).
2. **Пошагового «step-through по ходам агента» и брейкпоинтов на tool-calls в omp
   НЕТ.** Но есть все *примитивы*, из которых такое собирается: `tool_call`-хук с
   подменой аргументов (`shared-events.ts:325`), checkpoint/rewind через ветвление
   сессии (`tools/checkpoint.ts:53`, `agent-session.ts:10550`), кольцевой буфер
   сырых SSE-кадров (`raw-sse-buffer.ts:254`), запись/проигрывание экрана
   (`stream/recording.ts:97`). То есть в omp отлаживается **программа**, а
   наблюдаемость **агента** решена сбоку — трейсами и бандлами.
3. **Лицензия MIT** (`LICENSE`, 23 строки, `Copyright (c) 2025 Mario Zechner`,
   `2025-2026 Can Bölük`, `2026 Stencil Labs, Inc.`) — код брать можно с
   атрибуцией. Наш форк происходит из той же линии (`Cargo.toml:35-38`
   workspace authors = Stencil Labs, repository = `can1357/oh-my-pi`), так что
   формально мы уже под MIT-обязательствами — нужно только вести
   `THIRD-PARTY-NOTICES` при дословном переносе.

---

## 1. Что это такое и как этим пользуется человек

### 1.1. Два разных «debugger»

| | `debug` **tool** (для модели) | `/debug` **селектор** (для человека) |
|---|---|---|
| Вход | `packages/coding-agent/src/tools/debug.ts:543` | `packages/coding-agent/src/debug/index.ts:77` |
| Триггер | модель вызывает tool `debug` | `Shift+Ctrl+D` (`packages/tui/src/tui.ts:2952`) или `/debug` (`slash-commands/builtin-lifecycle.ts:522`) |
| Что делает | запускает/цепляет DAP-адаптер, ставит брейкпоинты, степает, читает переменные/память | открывает меню из 13 маршрутов: бандл отчёта, просмотр логов, raw SSE, системная инфа, состояние терминала, проба протоколов, JSC-инспектор, экспорт транскрипта, чистка кэша артефактов |
| Кому | агенту, который «должен уметь gdb» | разработчику omp при багрепорте |
| Гейт | `debug.enabled` (default `true`) — `tools/settings.ts:545-553` | всегда доступен в TUI |

Это важно для оценки переносимости: **это два независимых продукта с общей
судьбой только в документации** (`docs/tools/debug.md:3` прямо говорит, что
«adjacent debug UI code reuses the same subsystem for logs, raw SSE capture,
reports, profiling, and system diagnostics» — но фактически общий у них только
каталог `src/debug/`).

### 1.2. UX «отладчика программы» (то, чем гордится README)

`README.md:149-155` («03 · Drives a real debugger»): «C binary segfaults: the agent
attaches lldb, steps to the bad pointer, reads the frame. A Go service hangs: it
attaches dlv and walks the goroutines. A Python process is wedged: debugpy, pause,
inspect, evaluate. Most agents are still sprinkling print statements.»

Модель видит один tool с `action`-switch (28 действий,
`tools/debug.ts:69-97`) и короткий промпт из 4 строк
(`prompts/tools/debug.md`):

```
Debugger access. Prefer over bash for program state, breakpoints, stepping, or thread inspection.
Only one active session at a time. `program` is a target path, not a shell command.
Directories need a directory-capable adapter (e.g. `dlv`).
```

Типичный сценарий (из `examples` в `tools/debug.ts:567-576`):

```
1. debug(action: "launch", program: "./my_app")
2. debug(action: "set_breakpoint", file: "src/main.c", line: 42)
3. debug(action: "continue")
4. If the program appears hung: debug(action: "pause")
5. Inspect state with `threads`, `stack_trace`, `scopes`, and `variables`
```

В TUI каждый вызов рисуется карточкой с гридом «Session / Adapter / Status /
CWD / Program / Stop reason / Frame / Instruction pointer / Location»
(`packages/tui/src/tools/debug.ts:68-95` — `formatSessionSnapshot`,
`:185-244` — `debugToolRenderer`, `mergeCallAndResult: true`, `inline: true`).

### 1.3. UX «инспектора omp» (`/debug`)

Меню — плоский `SelectList` из 13 строк (`debug/index.ts:41-64`):

| value | Что делает | Где |
|---|---|---|
| `open-artifacts` | открыть папку артефактов сессии | `index.ts` (`#handleOpenArtifacts`) |
| `performance` | CPU-профиль + «воспроизведи баг, нажми Enter» + бандл | `debug/index.ts:151` |
| `work` | флеймграф планировщика за 30 с → SVG в `/tmp` | `debug/index.ts:233` |
| `dump` | бандл отчёта сразу | `debug/index.ts:257` |
| `memory` | `heapStats()` после GC → `memory.json` + бандл | `debug/index.ts:290` |
| `logs` | полноэкранный вьюер логов | `index.ts:337` → `tui/src/apps/debug/log-viewer.ts` |
| `raw-sse` | live-вьюер сырых SSE-кадров провайдера | `index.ts:381` → `tui/src/apps/debug/raw-sse.ts` |
| `remote-debugger` | поднять JavaScriptCore-инспектор Bun | `index.ts:409` → `debug/remote-debugger.ts:105` |
| `system` | OS/arch/CPU/RAM/версии/cwd/shell/terminal | `debug/system-info.ts` |
| `terminal` | subprotocols, геометрия, scrollback | `tui/src/apps/debug/terminal-info.ts` |
| `protocols` | тест уведомлений/ссылок/графики + sample image | `tui/src/apps/debug/protocol-probe.ts` |
| `transcript` | дамп видимого TUI-транскрипта в `/tmp/*.txt` | `modes/controllers/command-controller.ts:452` |
| `clear-cache` | удалить артефакт-папки старше 30 дней | `debug/report-bundle.ts` (`clearArtifactCache`) |

Ключевое: это **не «отладка в смысле breakpoint», а «сбор багрепорта»** —
основной артефакт `omp-report-<ts>.tar.gz` (`report-bundle.ts:85-100`):

```
session.jsonl      — транскрипт текущей сессии
artifacts/         — рекурсивно, включая вложенные транскрипты субагентов
logs.txt           — до 1000 строк из логов ВСЕХ процессов за сегодня
system.json        — OS/arch/CPU/RAM/версии
env.json           — санитизированное окружение
config.json        — resolved settings
profile.cpuprofile / profile.md  — только в performance
memory.json        — только числовые счётчики, никогда heap snapshot
raw-sse.txt        — кольцевой буфер SSE провайдера
work.folded/.md/.svg — профиль планировщика
```

---

## 2. Архитектура

### 2.1. Карта модулей

```
packages/coding-agent/src/
├── tools/debug.ts                986  ← единственный вход для модели (28 actions)
├── prompts/tools/debug.md          4  ← промпт модели
├── dap/
│   ├── client.ts                1060  ← транспорт DAP: spawn/connect, framing, request/event loop
│   ├── session.ts               1876  ← менеджер сессий: дерево root/child, брейкпоинты, stop-state
│   ├── config.ts                 418  ← выбор адаптера + загрузка dap.json из 6 мест
│   ├── defaults.json             ~250 ← 14 встроенных адаптеров (gdb, lldb-dap, dlv, debugpy, ...)
│   ├── types.ts                  615  ← формы DAP request/response/capabilities
│   └── index.ts                    4  ← реэкспорт
├── jsonrpc/message-framing.ts    198  ← Content-Length фрейминг, ОБЩИЙ для LSP и DAP
├── debug/
│   ├── index.ts                  603  ← селектор `/debug` (UI-only)
│   ├── report-bundle.ts          384  ← .tar.gz бандл + источник логов + чистка кэша
│   ├── profiler.ts               185  ← node:inspector CPU + bun:jsc heapStats
│   ├── system-info.ts            111  ← снимок системы + sanitizeEnv
│   └── remote-debugger.ts        151  ← bun:jsc startRemoteDebugger (одноразовый)
└── stream/recording.ts + player.ts    ← `/record` + `omp play` (запись/реплей экрана)

packages/tui/src/
├── tools/debug.ts                244  ← рендерер карточки debug-вызова
└── apps/debug/
    ├── log-viewer.ts            1219  ← полноэкранный вьюер логов (фильтр/выделение/копирование)
    ├── raw-sse.ts                499  ← вьюер SSE-кадра
    ├── raw-sse-buffer.ts         457  ← bounded ring-буфер кадров
    ├── viewer-frame.ts           174  ← общий chrome полноэкранных вьюеров
    ├── log-formatting.ts          53
    ├── terminal-info.ts          115
    └── protocol-probe.ts         258
```

Итого ~6.2k строк TS на «debugger» в узком смысле (без TUI-вьюеров).

### 2.2. Как встроено в agent loop

**Почти никак.** `DebugTool` — обычный `AgentTool` (`tools/debug.ts:543-986`):

- `readonly concurrency = "exclusive"` (`:579`) — планировщик блокирует параллельные
  вызовы debug-тула;
- `readonly loadMode = "discoverable"` (`:580`) — монтируется как `xd://debug`;
- `static createIf()` (`:586`) возвращает `null` если `debug.enabled=false`, и
  `tools/index.ts:743` перепроверяет тот же сеттинг при фильтрации ростера;
- `approval` зависит от action (`:545-550`): `DEBUG_READONLY_ACTIONS`
  (`:57-67`: `output`, `threads`, `stack_trace`, `scopes`, `variables`,
  `disassemble`, `read_memory`, `loaded_sources`, `modules`, `sessions`) → tier
  `read`; всё остальное → `exec`.

То есть отладчик **не перехватывает** loop — он просто один из инструментов,
который модель дёргает как `bash`. Никакого специального события
`agent_debug_pause` в `AgentEvent` (`packages/agent/src/types.ts:1253-1275`) нет.

### 2.3. Ключевые абстракции

**`DapClient`** (`dap/client.ts:54`) — один адаптер-процесс или сокет:

- `static spawn({adapter, cwd})` (`:106`) — три режима транспорта:
  `stdio` (пайпы процесса, `detached: true` + `NON_INTERACTIVE_ENV`, чтобы
  debuggee не получил controlling terminal и не словил SIGTTIN — комментарий
  `:117-121`), `socket` (unix-сокет на Linux / callback-listener на macOS),
  `tcp` (резерв loopback-порта, подстановка `${port}` в argv, ожидание listen);
- `static connect()` (`:136`) — подключение дочерней сессии к уже поднятому
  root-TCP-серверу (нужно для `startDebugging`);
- `sendRequest(command, args, signal, timeoutMs)` (`:432`) — с
  `DEFAULT_REQUEST_TIMEOUT_MS = 30_000` (`:39`) и
  `WRITE_MESSAGE_TIMEOUT_MS = 30_000` (`:45`, «зависший stdin адаптера раньше
  вешал клиент навсегда → dispose, чтобы следующий запрос упал быстро»);
- `waitForEvent(event, predicate, signal, timeoutMs)` (`:387`) — подписка
  «до отправки команды» с отменой по abort/закрытию транспорта;
- `onReverseRequest("runInTerminal" | "startDebugging")` — обратные запросы
  адаптера;
- `isAlive()` (`:349`), `dispose()` (`:580`).

**`DapSessionManager`** (`dap/session.ts:284`) — синглтон
(`export const dapSessionManager = new DapSessionManager()` в конце файла):

- дерево сессий: `parentSessionId` / `childSessionIds` (`DapSession` интерфейс,
  `:66-99`), `#activeSessionId` — «кто сейчас в фокусе»;
- `#ensureLaunchSlot()` (`:1323`) — **жёстко одна root-сессия**:
  `Debug session <id> is still active. Terminate it before launching another.`;
- кэш состояния в `DapSession`: `breakpoints: Map<path, DapBreakpointRecord[]>`,
  `functionBreakpoints`, `instructionBreakpoints`, `dataBreakpoints`,
  `outputChunks` (ring, `MAX_OUTPUT_BYTES = 128 KiB`, `:115`), `stop`
  (`DapStopLocation`), `threads`, `lastStackFrames`, `capabilities`,
  `needsConfigurationDone`, `configurationDoneSent`;
- тайминги: `IDLE_TIMEOUT_MS = 10 min` / `CLEANUP_INTERVAL_MS = 30 s` (`:112-113`),
  `HEARTBEAT_INTERVAL_MS = 5 s` (`:114`), `STOP_CAPTURE_TIMEOUT_MS = 5 s` (`:116`);
- `buildSummary()` (`:254`) → `DapSessionSummary` (`dap/types.ts:563-589`) —
  ровно то, что видит модель и TUI.

**`DapAdapterConfig`** (`dap/config.ts` + `defaults.json`) — данные, не код:

```json
"lldb-dap": {
  "command": "lldb-dap", "args": [],
  "languages": ["c","cpp","objc","swift","rust","zig"],
  "fileTypes": [".c",".cpp",".rs",".zig"],
  "rootMarkers": ["Package.swift","Cargo.toml","Makefile","CMakeLists.txt","build.zig"],
  "launchDefaults": { "request": "launch", "stopOnEntry": true },
  "attachDefaults": { "request": "attach" }
}
```

14 адаптеров: `gdb`, `lldb-dap`, `codelldb`, `debugpy`, `dlv`, `js-debug-adapter`,
`netcoredbg`, `kotlin-debug-adapter`, `rdbg`, `php-debug-adapter`,
`bash-debug-adapter`, `dart-debug-adapter`, `flutter-debug-adapter`,
`elixir-ls-debugger`.

Пользовательский конфиг — `dap.json|.dap.json|dap.yaml|.dap.yaml|dap.yml|.dap.yml`,
мержится из 6 слоёв: cwd → проектные конфиг-диры (`.omp/`, `.claude/`, `.codex/`,
`.gemini/`) → пользовательские → плагины → home-fallback (`config.ts:110`).
Автовыбор: `selectLaunchAdapter()` (`:357`, ранжирование extension-match →
root-marker → предпочтение нативных), `selectAttachAdapter()` (`:374`,
`port` ⇒ debugpy первым), `resolveLaunchOverrides()` (`:403`, спец-кейс dlv:
`mode: "debug"` для каталогов/`.go`, `mode: "exec"` для бинарей).

**`MessageFramer`** (`jsonrpc/message-framing.ts:78`) — Content-Length
фрейминг, **общий для LSP и DAP** (комментарий `:1-8`): `MAX_HEADER_BYTES = 16 KiB`
(`:17`), `MAX_CONTENT_BYTES = 256 MiB` (`:18`), `encodeMessageFrame()` (`:35`).

---

## 3. Механизмы детально

### 3.1. Перехват состояния программы: события, не хуки

Никаких «хуков» в loop. Всё — подписка на DAP-события в
`#registerSession()` (`dap/session.ts:1334`), обработчики `:1406-1440`:

| DAP event | Что делает менеджер |
|---|---|
| `output` | пишет чанк в `session.outputChunks`, режет спереди по `MAX_OUTPUT_BYTES` (`:1406`) |
| `initialized` | `initializedSeen = true` (`:1409`) |
| `stopped` | фиксирует `stop{reason, threadId, frameId, source, line, column}`, переключает `#activeSessionId` на остановившуюся сессию (`:1415`) |
| `continued` | сбрасывает `stop` (`:1420`) |
| `exited` | `exitCode` (`:1426`) |
| `terminated` | `status = "terminated"`, реактивация родителя, разрешение tree-waiter'ов (`:1432`) |

Плюс `setInterval(HEARTBEAT_INTERVAL_MS)` на `client.isAlive()` → `terminated`
(`:1448-1457`) и `client.proc.exited.finally(...)` → dispose + `#reactivateAfterTermination`.

### 3.2. Обратные запросы адаптера (reverse DAP)

`runInTerminal` (`:1369`) — адаптер просит запустить debuggee: спавнит его
через `ptree.spawn()` **detached**, дренирует stdout в bounded-кэш
(`drainTerminalStdout()`, `:234`), возвращает `{processId}`.

`startDebugging` (`:1392`) — адаптер просит **дочернюю** сессию (js-debug
делает так для `[worker N]`): `#startChildSession()` (`:1191`) коннектится к
root-TCP-серверу, делает `initialize` → переносит **все root-брейкпоинты**
дочерней сессии до её `configurationDone` (`#applyRootBreakpointsToSession`,
`:1239`) → рекурсивно вешает те же обработчики. Фокус переходит к ребёнку
только если ни одна живая сессия ещё не стоит на стопе (`:1440-1446`,
комментарий про `[worker N]`).

### 3.3. Step-through — как это реально работает

Ключевая тонкость, которую стоит украсть: **подписка строго ДО команды**
(`#prepareStopOutcome`, `:1582`; комментарий `:1566-1568`: «Reset state and
subscribe BEFORE sending the step command to avoid missing events that arrive in
the same buffer as the response»).

```
continue/stepIn/stepOut/stepOver
  → session.stop = {}; lastStackFrames = []; status = "running"
  → outcomePromise = #prepareStopOutcome(session)     // waiter в #treeOutcomeWaiters
  → sendRequest("next"|"stepIn"|"stepOut"|"continue")
  → #awaitStopOutcome(session, outcomePromise)        // :1620
       ├ stopped  → #fetchTopFrame() (5s budget) → {state:"stopped", snapshot}
       ├ terminated → {state:"terminated"}
       └ таймаут  → {state:"running", timedOut:true}  ← НЕ ошибка
```

Waiter привязан к **root id дерева** (`#treeOutcomeWaiters`, `:1582-1612`),
поэтому остановка любого ребёнка разрешает ожидание родителя. `pause` (`:950`)
сначала подписывается на `stopped`, потом шлёт `pause`, и переиспользует
закэшированный стоп, если программа уже стояла.

`#fetchTopFrame()` (`:1546`) — после остановки тянет `stackTrace` и кэширует
верхний фрейм, чтобы `scopes`/`variables`/`evaluate` без явных id работали
(«defaults to the current stopped frame»).

### 3.4. Что можно менять на лету

| Действие | Можно ли на ходу |
|---|---|
| Брейкпоинты (source/function/instruction/data) | да, синхронизируются по всему дереву (`#syncBreakpointTree`, `:467`; сериализация мутаций — `breakpointMutationQueue`, «см. `#serializeBreakpointMutation`») |
| Условия и hit-count | да (`condition`, `hit_condition` в схеме) |
| Память процесса | да, `write_memory` (base64, capability-gated) |
| Выражения | да, `evaluate` с `context: "repl"` → можно гнать **сырые команды отладчика** (`info registers`) |
| Произвольный DAP-запрос | да, `custom_request` (`:949`) |
| Потоки/фреймы/скоупы | только чтение |
| Запуск нового root-процесса | **нет** — одна root-сессия за раз (`:1323`) |

### 3.5. Диагностика самого агента: три разных механизма

**(a) Ring-буфер сырых SSE провайдера** — `RawSseDebugBuffer`
(`tui/src/apps/debug/raw-sse-buffer.ts:254`).

Подключение: `AgentSession` создаёт буфер (`agent-session.ts:1919`) и вешает
его на **два интерсептора ядра агента** (`agent-session.ts:1927,1942` →
`agent.setProviderResponseInterceptor` / `setRawSseEventInterceptor`,
`packages/agent/src/agent.ts:1150,1154`). Сам интерсептор — тривиальный
`notifyRawSseEvent()` (`packages/ai/src/utils/sse-debug.ts:6`) с контрактом
«наблюдатель владеет массивом `raw`, не мутирует и не клонирует».

Бюджеты (`raw-sse-buffer.ts:4-6`): `MAX_RAW_SSE_EVENTS = 1000`,
`MAX_RAW_SSE_CHARS = 512_000`, `MAX_RAW_SSE_EVENT_CHARS = 64_000`. Обрезка
трёхступенчатая (`trimRawLines()`, `:199`): влезает → отдать по ссылке;
не влезает → **сжать схемы tools внутри `data:` JSON до 200-символьных
превью** (payload остаётся валидным JSON); всё ещё не влезает → head+tail
с маркерами `: omp-debug-elided chars=N` и `: omp-debug-truncated originalChars=N`.
Эвикция — head-index ring (`#head`, `:383`) без `shift()`.

Плюс `recordResponse()` пишет SSE-комментарий-метадату
(`: formatRawSseResponseComment`): `ts`, `status`, `provider`, `model`, `api`,
`requestId`, `transport`. Это и есть «что реально ушло/пришло по проводу».

**(b) Бандл отчёта** — `createReportBundle()` (`debug/report-bundle.ts:85`).
Читает session JSONL, рекурсивно артефакты текущей сессии (включая
транскрипты субагентов), до 1000 строк из **каждого** лог-файла за сегодня
(`collectSameDayLogs()`, `:218`; PID-квалифицированные имена — чтобы достать
лог упавшего процесса из следующего запуска), `system.json`,
`env.json` (санитайз по **именам** переменных — `system-info.ts`,
`SENSITIVE_HEADER_PATTERN` в `ai/src/utils/http-inspector.ts:29` для заголовков),
`config.json`. Кэп на интерактивное чтение логов: `MAX_LOG_LINES = 5000`,
`MAX_LOG_BYTES = 2 MiB` (`:17,20`).

**(c) Запись/реплей экрана** — `stream/recording.ts:97` (`parseRecording`),
`stream/player.ts` (`playRecording`). Формат `.ompcast` — JSONL, asciicast-подобный:
первая строка заголовок `{"ompcast":1,"cols":120,"rows":40,...}`, дальше
`[ms, frame]` где frame — `viewport` / `patch` / `history` / `reset`.
То есть **полный реплей TUI-сессии**, включая правки отдельных строк
(`patch.ops: [[39, "…"]]`).

### 3.6. Чего НЕТ (важно для оценки)

- ❌ Step-through по ходам агента («остановись после turn N, покажи промпт,
  дай поправить tool-call»).
- ❌ Брейкпоинт на tool-call как UX-фича (только как extension-хук, см. §5.4).
- ❌ Дамп «что именно ушло в модель» как first-class артефакт. Ближайшее —
  `raw-sse.txt` (сырые кадры, не нормализованный payload) и `PI_REQ_DEBUG=1`
  (`ai/src/utils/request-debug.ts:4`) — файловый дамп request/response только
  для отдельных провайдеров (`cursor.ts:1294`, `openai-codex-responses.ts:4193`).
- ❌ Инспекция живого состояния агента из TUI (аналог нашего `/context`) —
  вместо этого есть `/context`-подобный `Agent Hub` (`docs/agent-hub.md`,
  `tui/src/overlays/agent-hub.ts:327`) для **субагентов**.

---

## 4. Лицензия → можно ли брать код

**Файл `LICENSE` (23 строки), дословно:**

```
MIT License

Copyright (c) 2025 Mario Zechner
Copyright (c) 2025-2026 Can Bölük
Copyright (c) 2026 Stencil Labs, Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

Проверки:
- `package.json:5` → `"license": "MIT"`;
- `packages/coding-agent/LICENSE`, `packages/tui/LICENSE` — тот же MIT, те же три
  копирайта (per-package дубли);
- `Cargo.toml:35-38` (наш форк): `license = "MIT"`, `authors = ["Stencil Labs, Inc."]`,
  `repository = "https://github.com/can1357/oh-my-pi"`;
- `THIRD-PARTY-NOTICES.txt` — генерируемый агрегат, «first-party licensing grants
  no rights in third-party code»; под MIT-кодом самого omp отдельных записей нет
  (значит, first-party).

**Вывод:** код брать **можно**, включая дословный перенос и перевод на Rust
(MIT разрешает derivative works). Обязательство одно: сохранить copyright-нотис
и текст permission-абзаца в «all copies or substantial portions». Практически:
- при **дословном** порте файла — шапка `// Ported from oh-my-pi (MIT).
  Copyright (c) 2025 Mario Zechner, 2025-2026 Can Bölük, 2026 Stencil Labs, Inc.`
  + запись в `THIRD-PARTY-NOTICES.txt` нашего репо;
- при **переосмыслении паттерна** (архитектура, бюджеты, последовательность
  handshake) — обязательств нет, но атрибуция в `.research/` всё равно полезна.

Риск: `dap/defaults.json` — данные (имена команд, флаги). Это не творческая
часть, но формально часть пакета → та же MIT-шапка, если копируем файл целиком.

---

## 5. Переносимость в наш форк (Rust)

### 5.0. Что у нас уже есть (точки крепления)

| Наше | Где | Что даёт |
|---|---|---|
| `AgentEvent` (полный набор: `TurnStart/End`, `ToolExecutionStart/Update/End`, `MessageUpdate`) | (наш) `crates/pa-agent/src/types.rs:583-621` | готовый event-stream для любого инспектора |
| `before_tool_call` / `after_tool_call` | (наш) `crates/pa-agent/src/agent.rs:96-97`, `types.rs:629-645` | точка перехвата tool-call |
| daemon + worker + wire-протокол, `Attach` с `resume_cursor` | (наш) `crates/pa-types/src/daemon/command.rs:246-266`, `DaemonReplayInfo` в `daemon/mod.rs:67` | живая подписка на события сессии **из другого процесса** |
| `pa-lsp` на `async-lsp` (Content-Length поверх tokio-пайпов) | (наш) `crates/pa-lsp/src/client.rs:289`, `Cargo.toml:23-27` | 80% работы DAP-транспорта уже сделано (тот же base protocol) |
| `debug_log.rs` — ротация `debug.log → .1 → …` | (наш) `crates/pa-daemon/src/debug_log.rs:47-115,227` | источник для вьюера логов |
| `/logs` панель | (наш) `crates/pa-tui/src/info_commands.rs:182`, `session_ui/commands.rs:709` | UI-каркас уже есть |
| `InfoPanel` + `InfoContent::{Rows,Markdown}` | (наш) `crates/pa-tui/src/info_panel.rs:25-110` | куда вставлять новые панели |
| `/cache` статистика | (наш) `crates/pa-tui/src/info_commands/cache.rs:124` + `DaemonCommand::GetCacheStats` (`command.rs:655`) | образец «daemon-запрос → панель» |
| golden replay против TS-корпуса | (наш) `crates/pa-core/src/tools/golden_replay.rs:26` + `tests/golden/corpus/*.json` | механизм trace-ассертов |
| `ScriptedEngine` (детерминированный faux) | (наш) `crates/pa-daemon/src/engine/scripted.rs:28` | воспроизводимые прогоны |
| `getWorkProfile`/CPU-профайлер | ❌ нет | только `tracing` + `tokio-console` |

### 5.1. Паттерны, которые переиспользуемы (и куда встают)

| # | Паттерн omp | Куда у нас | Цена |
|---|---|---|---|
| P1 | **Подписка-до-команды** для stop/step (`session.ts:1566-1582`) | любой `wait_for_*` в daemon: `tokio::sync::broadcast::Receiver` создаётся до `send()`. Уже частично есть в `input_pause_lease`, но правило стоит зафиксировать в `AGENTS.md` | 0 (дисциплина) |
| P2 | **Tree outcome waiter**: ожидание «стоп где угодно в дереве сессий» (`#treeOutcomeWaiters`) | ожидание «любой ребёнок закончил» в `rlm_children` / `agents_view` | S |
| P3 | **Bounded ring + 3-ступенчатая обрезка** (`raw-sse-buffer.ts:199-227`) | новый `pa-core/src/trace/ring.rs` для трейсов прогонов | S |
| P4 | **Данные вместо кода: `defaults.json` + 6-слойный мерж** (`dap/config.ts:110`) | любой реестр адаптеров/провайдеров; у нас похожее в `pa-daemon/src/scheduling_catalog.rs` | S |
| P5 | **Action-switch tool с approval по action** (`tools/debug.ts:545-550`) | если делаем `debug`/`trace` tool — один tool вместо пяти | XS |
| P6 | **`exclusive` concurrency + единственная root-сессия** (`:579,1323`) | защита от параллельных отладочных сессий | XS |
| P7 | **Heartbeat + idle cleanup loop** (`:112-116,1448`) | у нас есть в `boot_reap`/`lease`; паттерн знаком | — |
| P8 | **Report bundle** (tar.gz: session + logs + system + env + config) | `pa-cli` команда `allspark doctor --bundle` | S |
| P9 | **Checkpoint/rewind как ветвление + squash** (`tools/checkpoint.ts:53`, `agent-session.ts:10550`) | у нас `branch`/`branch_with_summary` уже есть (`session/manager_ext.rs:199,221`) | M |

### 5.2. Что реалистично портировать как код

| Компонент | TS LOC | Rust LOC (оценка) | Сложность | Комментарий |
|---|---:|---:|---|---|
| `MessageFramer` (`message-framing.ts:78-198`) | ~120 | ~150 | XS | Уже покрыто `async-lsp`; отдельный крейт нужен только если хотим DAP без async-lsp |
| `DapClient` (`client.ts`) | 1060 | ~1200-1400 | **L** | tokio + `tokio::process` + `serde_json`. Три транспорта (stdio/socket/tcp). Reverse-requests — обычный `oneshot`-роутер |
| `DapSessionManager` (`session.ts`) | 1876 | ~1600-2000 | **L** | Дерево сессий, брейкпоинт-кэш, waiter'ы, cleanup. Много мелкой семантики (dlv mode, js-debug children) |
| `dap/config.ts` + `defaults.json` | 418 + данные | ~400 + JSON | **S** | Почти дословно: `serde` + мерж слоёв + ранжирование |
| `tools/debug.ts` (action-switch) | 986 | ~900 | **M** | Много форматирования в текст; механика простая |
| `debug/report-bundle.rs` | 384 | ~450 | **S** | `tar` + `flate2`; санитайз env/заголовков |
| `debug/system-info.rs` | 111 | ~150 | **XS** | `sysinfo` или ручной `/proc` |
| `RawSseDebugBuffer` | 457 | ~400 | **S-M** | Ring + триммер; нужен хук в `pa-ai` stream |
| `log-viewer` (TUI) | 1219 | ~800-1000 | **M** | У нас ratatui, не native-node — рендер переписывается, модель (фильтр/выделение/expand/copy) переносится |
| **Итого «DAP целиком»** | ~3.4k | **~4.5-5.5k** | | **≈3-4 недели** одного инженера с тестами |

Оценка «DAP целиком» реалистична потому, что `pa-lsp` — рабочее доказательство,
что мы умеем держать долгоживущий процесс, гонять Content-Length JSON-RPC,
делать `initialize`-handshake и abort/timeout. DAP — тот же base protocol,
отличается только семантика (`configurationDone`, reverse-requests, дерево сессий).

### 5.3. Что НЕ переносится (Bun/TS-специфика)

| omp | Почему не переносится | Что вместо |
|---|---|---|
| `remote-debugger.ts` (`bun:jsc` `startRemoteDebugger`) | API Bun/JavaScriptCore, одноразовый, без stop-handle | не нужно (у нас нет JSC); аналог — `tokio-console`/`tracing` |
| `profiler.ts` (`node:inspector/promises` CPU, `bun:jsc` `heapStats`) | V8/JSC-специфичные форматы | `tracing`-спаны + `pprof`-rs; heap — `jemalloc`/`dhat` (у нас уже упомянут в `.research/fork-testing-matrix.md` как G7) |
| `getWorkProfile(30)` + SVG-флеймграф | `@oh-my-pi/pi-natives` (N-API) | `pprof` + `inferno` (в omp `inferno` уже в `about.toml`; у нас зависимость добавляется) |
| TUI `native/node` `describe`-дерево (`tui/src/native/*`) | Bun-нативный рендер-пайплайн с `Memo`/`NativeNode`/`TspSpan` | ratatui `Widget`; переносим **модель**, не рендер |
| `SelectList`/`OverlayPanel`/`TranscriptBlock` | pi-tui компоненты | у нас `menu_panel.rs`, `choice_picker.rs`, `info_panel.rs` |
| `Bun.file().text()`, `Bun.write`, `Bun.JSONL.parseChunk` | Bun API | `std::fs`, `serde_json` |
| `ptree.spawn` (process-tree) | pi-utils над Bun | `tokio::process::Command` + `process_group(0)` |
| `copyToClipboard`, `openPath` | хост-интеграция | у нас `clipboard.rs` есть |

### 5.4. Конкретные кандидаты внедрения (приоритет)

#### 🥇 C1. Trace-capture + trace-assert (для black-box QA) — **P0**
**Что:** нормализованный дамп траектории прогона: per-turn `{prompt_digest,
system_prompt_digest, tools_digest, model, params}`, per-tool-call `{name, args,
result_summary, is_error, duration_ms}`, usage/латентности, стоп-причина.
Плюс `allspark trace diff a.json b.json` и trace-ассерты в стиле CORE.
**Из omp берём:** идею `RawSseDebugBuffer` (ring + нормализация + бюджеты),
`createReportBundle` (что кладём в артефакт), `notifyRawSseEvent` (тривиальный
хук в stream-пайплайн).
**Куда:** `pa-core/src/trace/` + хук в `pa-agent` (у нас `AgentEvent` уже несёт
`ToolExecutionStart{args}` — `types.rs:606-611`, нужно только сериализовать);
команда в `pa-cli`; writer — рядом с `session_store`.
**Почему P0:** это прямой ответ на нашу текущую задачу (см. §6) и
переиспользует уже существующие `AgentEvent` + `golden_replay`.

#### 🥈 C2. `/debug` инспектор в pa-tui — **P0**
**Что:** панель `Shift+Ctrl+D` / `/debug` с 4-5 маршрутами:
`logs` (полноэкранный вьюер с фильтром/expand/copy поверх `debug_log.rs`),
`trace` (последний трейс прогона: turns/tool-calls/usage),
`stats` (сессия + кэш + контекст — склейка существующих `cache_rows`/
`context_tree_rows`), `system`, `dump` (бандл).
**Из omp берём:** структуру меню (`debug/index.ts:41-64`), модель вьюера логов
(`log-viewer.ts:29-36`: `INITIAL_LOG_CHUNK=50`, `LOAD_OLDER_CHUNK=50`, `MAX_LOG_ENTRIES=50_000`,
фильтр-строка, `expand`, `copy`, «load older» с маркером
`### WARNING - Logs above are older than current session!`), `viewer-frame.ts`
(chrome), `report-bundle` (набор файлов).
**Куда:** `pa-tui/src/debug_view/` + `session_ui/commands.rs` (новый arm рядом с
`"logs"` на `:709` и `"cache"` на `:641`), данные — через
`DaemonCommand::GetDebugDump` (новый вариант рядом с `GetCacheStats` `command.rs:655`).
**Почему P0:** у нас уже есть `InfoPanel` + `/logs` + `debug_log` ротация →
это в основном «сделать полноэкранный вьюер и добавить 3 маршрута».

#### 🥉 C3. Turn-level checkpoint/rewind («шаг назад») — **P1**
**Что:** модель (или человек) ставит чекпойнт, работает, затем «откатывается»:
ветка-исследование схлопывается в summary, лист переезжает на чекпойнт.
**Из omp берём:** `tools/checkpoint.ts:53,89` (два tool'а, ~130 строк на двоих),
`agent-session.ts:10550` (`#applyRewind` → `branchWithSummary`),
`:10507` (`#enforceRewindBeforeYield` — «нельзя сдаться, не вызвав rewind»,
system-warning + перезапуск хода).
**Куда:** `pa-core/src/tools/checkpoint.rs` + `session/manager_ext.rs` — у нас уже
есть ровно те два примитива, на которых стоит omp-rewind:
`SessionManager::branch(branch_from_id)` (`crates/pa-core/src/session/manager_ext.rs:199`)
и `branch_with_summary(...)` (`:221`), плюс `set_leaf_id()` в
`session/manager/append.rs:354`. Т.е. C3 — это в основном два tool-а и
энфорс-петля, а не новый движок.
**Почему P1:** даёт «step back» UX почти бесплатно и напрямую улучшает
QA-цикл (неудачное исследование не засоряет контекст).

#### C4. Tool-call breakpoint с редактированием аргументов — **P1**
**Что:** пауза перед выполнением выбранного tool-call, показ args, возможность
переписать args или заблокировать.
**В omp это есть как extension-хук:** `ToolCallEventResult { block, reason,
input, additionalContext }` (`extensibility/shared-events.ts:325-352`) — причём
`input` **перепроверяется по схеме и становится тем, что реально исполняется,
показывается, персистится и апрувится** (комментарий `:336-346`).
**У нас дырка:** `BeforeToolCallResult` = только `{block, reason}`
(`crates/pa-agent/src/types.rs:629-632`) — **нет `input`-override**.
**Патч:** +1 поле в `BeforeToolCallResult`, применение в
`agent_loop/mod.rs:76` до `ToolExecutionStart`, ре-валидация по схеме tool'а.
**Цена:** XS (полдня) для `input`; M для UI-паузы (нужен round-trip
daemon→TUI→daemon с таймаутом, у нас есть `input_pause_lease` как образец).
**Почему P1:** это и есть «настоящий debugger агента» — и он встаёт на уже
существующий хук, без нового слоя.

#### C5. `pa-dap` — настоящий отладчик программ — **P2 (стратегический)**
**Что:** tool `debug` с 20+ actions, 14 адаптеров, автовыбор, дерево сессий.
**Из omp:** `dap/*` целиком (см. таблицу §5.2).
**Куда:** новый крейт `crates/pa-dap` по образцу `pa-lsp` (зависимости:
`tokio`, `serde_json`, `async-lsp`-фрейминг или свой 150-строчный framer),
wire-регистрация в `session_engine/engine.rs:537` рядом с `LspTool`.
**Почему P2:** 3-4 недели и высокая дифференциация («агент умеет gdb»), но к
текущей QA-задаче отношения не имеет. Начинать стоит только после C1/C2.

#### C6 (bonus). Report bundle `allspark doctor --bundle` — **P2**
384 строки TS → ~450 Rust. `tar`+`flate2`, набор файлов как в §1.3.
Отдельная ценность: санитайз env **по именам** (не по значениям — omp честно
предупреждает, что это не защита от секретов в значениях,
`docs/tools/debug.md` → Side Effects) и «не кладём heap snapshot никогда»
(там же: старые memory-отчёты с `heap.heapsnapshot` = credential-bearing).

---

## 6. Связь с нашей задачей black-box QA

Наша задача — «тестирование реальными задачами, выявление отклонений». В
`.research/blackbox-qa-best-practices.md` уже зафиксирован канон: **grade what the
agent produced, not the path it took** (Anthropic), но при этом trace/span-уровень
обязателен для *диагностики* (OpenAI traces→graders, CORE full-path). Разрыв
именно в инструменте: **чем снимать траекторию**.

Что даёт «debugger-слой» из omp как инструмент снятия траекторий:

| Нужно для trace-ассертов | Что даёт omp-паттерн | Наш статус |
|---|---|---|
| Точный список tool-call'ов с args | `AgentEvent::ToolExecutionStart{args}` уже есть (`pa-agent/types.rs:606`); omp-паттерн = просто сериализовать + bounded ring | есть событие, нет writer'а |
| «Что реально ушло в модель» (промпт + tools) | `raw-sse.txt` + `setOnModelCallSystemPrompt` (`agent.ts:1176`) + `PI_REQ_DEBUG` | нет; нужен C1 |
| Воспроизводимый реплей | `.ompcast` (`recording.ts:97`) — но это **экран**, не траектория; для траектории — `ScriptedEngine` (наш, `scripted.rs:28`) | faux есть, записи нет |
| Сравнение двух версий по траектории | `golden_replay.rs` + `deep_norm()` (нормализация tmp-путей/hash) | есть, но только для tool-вывода, не для траектории |
| Пошаговый разбор провала | `/debug` → `transcript` (дамп видимого TUI) — слабо; лучше — turn-by-turn viewer | нет |

**Ключевой вывод для QA:** omp-«debugger» **не является** инструментом снятия
траекторий — он либо отлаживает чужую программу (DAP), либо собирает багрепорт
(бандл). Траектории omp снимает **сбоку**: `raw-sse` + `session.jsonl` +
`AgentActivityIndex` (`activity/index.ts:208` — построение строк активности из
JSONL транскрипта, инкрементально по байтовому offset'у, `INITIAL_TAIL_BYTES =
256 KiB`, `MAX_ROWS_PER_AGENT = 256`).

Именно **`AgentActivityIndex`** — самый недооценённый кусок для нашей QA-задачи:
это готовый паттерн «читаем session JSONL с offset'а → нормализуем в строки
`{kind: response|tool|irc|lifecycle, title, summary, status, toolName, source:
live|transcript|irc}` → отдаём в UI/ассерты». Он уже работает и на живых
событиях, и на персистентном транскрипте (`setLive` / `sync`), т.е. **один и
тот же нормализатор для live-инспекции и для post-hoc trace-ассертов**. Это
ровно то, чего нам не хватает, чтобы trace-ассерты не разъезжались с тем, что
показывает TUI.

Практическая рекомендация: **C1 (trace-capture) строить не как «дамп SSE», а
как нормализатор в стиле `AgentActivityIndex`** — одна функция, два режима
(live из `AgentEvent`, offline из session JSONL). Тогда:
- trace-ассерт в CI читает `.jsonl` и не требует живого прогона;
- `/debug trace` в TUI читает те же строки из live-потока;
- golden-корпус (`golden_replay.rs:26`) расширяется третьим типом кейсов:
  не «tool-вывод», а «траектория прогона против faux-модели».

Анти-паттерны omp, которые нам **не** надо копировать:
- **Глобальный синглтон** `dapSessionManager` + «одна root-сессия» (`session.ts:1323`)
  — для QA нужно много параллельных прогонов; у нас уже daemon-per-session, и
  это правильнее;
- **`exclusive` concurrency** на debug-туле (`tools/debug.ts:579`) — блокирует
  параллельные прогоны в одном процессе;
- **UI-маршруты, не доступные модели** (`/debug` — только человек,
  `docs/tools/debug.md` → «Interactive selector values … not model-callable») —
  для QA-агента нужен машинный доступ, а не меню;
- **`/tmp/work-profile-<ts>.svg`** и прочие абсолютные `/tmp`-пути — у нас есть
  `platform::agent_dir()` и артефакт-директория сессии, надо туда.

---

## 7. Итоговая таблица решений

| # | Кандидат | Приоритет | Цена | Что берём | Риск |
|---|---|---|---|---|---|
| C1 | Trace-capture + trace-assert (нормализатор в стиле `AgentActivityIndex` + ring как `RawSseDebugBuffer`) | **P0** | M (1 нед.) | паттерн + бюджеты + формат бандла | низкий |
| C2 | `/debug` инспектор в pa-tui (logs/trace/stats/system/dump) | **P0** | M (1 нед.) | модель вьюера + структура меню | низкий |
| C3 | checkpoint/rewind (ветка-исследование → summary) | P1 | S (2-3 дня) | почти дословно 130 строк | низкий |
| C4 | tool-call breakpoint c `input`-override | P1 | XS+S | паттерн `ToolCallEventResult.input` | средний (нужен UI round-trip) |
| C5 | `pa-dap` (настоящий отладчик программ) | P2 | L (3-4 нед.) | код `dap/*` (MIT) | средний (адаптеры в среде) |
| C6 | `doctor --bundle` | P2 | S | `report-bundle.ts` | низкий |

**Рекомендация:** начать с C1+C2 одной волной (они делят нормализатор и
артефакт), C3/C4 — вторая волна (дают UX «шаг назад» и «брейкпоинт» для
агента), C5 — только если появится запрос «агент должен уметь gdb».

---

## Приложение: индекс цитат

**omp, DAP-ядро**
- `packages/coding-agent/src/tools/debug.ts:57` `DEBUG_READONLY_ACTIONS`
- `packages/coding-agent/src/tools/debug.ts:69-97` `debugActionSchema` (28 actions)
- `packages/coding-agent/src/tools/debug.ts:543` `class DebugTool`
- `packages/coding-agent/src/tools/debug.ts:545-550` approval по action
- `packages/coding-agent/src/tools/debug.ts:579-580` `exclusive` + `discoverable`
- `packages/coding-agent/src/tools/debug.ts:586` `createIf` (гейт `debug.enabled`)
- `packages/coding-agent/src/tools/debug.ts:603,793` `case "launch"` / `case "step_over"`
- `packages/coding-agent/src/dap/client.ts:39,45,47` таймауты
- `packages/coding-agent/src/dap/client.ts:54` `class DapClient`
- `packages/coding-agent/src/dap/client.ts:106,136` `spawn` / `connect`
- `packages/coding-agent/src/dap/client.ts:349,353,359,387,432,580` API
- `packages/coding-agent/src/dap/session.ts:66-99` `interface DapSession`
- `packages/coding-agent/src/dap/session.ts:112-116` тайминги и `MAX_OUTPUT_BYTES`
- `packages/coding-agent/src/dap/session.ts:254` `buildSummary`
- `packages/coding-agent/src/dap/session.ts:284` `class DapSessionManager`
- `packages/coding-agent/src/dap/session.ts:1323` `#ensureLaunchSlot` (одна root)
- `packages/coding-agent/src/dap/session.ts:1334` `#registerSession`
- `packages/coding-agent/src/dap/session.ts:1369,1392` reverse-requests
- `packages/coding-agent/src/dap/session.ts:1406-1432` обработчики событий
- `packages/coding-agent/src/dap/session.ts:1448-1457` heartbeat
- `packages/coding-agent/src/dap/session.ts:1546` `#fetchTopFrame`
- `packages/coding-agent/src/dap/session.ts:1566-1582` «подписка до команды»
- `packages/coding-agent/src/dap/session.ts:1620` `#awaitStopOutcome`
- `packages/coding-agent/src/dap/session.ts:1191,1239` child-сессии и брейкпоинты
- `packages/coding-agent/src/dap/config.ts:110,127,357,374,403` конфиг и автовыбор
- `packages/coding-agent/src/dap/types.ts:563-589` `DapSessionSummary`
- `packages/coding-agent/src/jsonrpc/message-framing.ts:17,35,78` фрейминг

**omp, диагностика**
- `packages/coding-agent/src/debug/index.ts:41-64` меню `/debug`
- `packages/coding-agent/src/debug/index.ts:77` `DebugSelectorComponent`
- `packages/coding-agent/src/debug/index.ts:151,233,257,290,337,381,409` обработчики
- `packages/coding-agent/src/debug/report-bundle.ts:17,20,85,218` бандл и кэпы
- `packages/coding-agent/src/debug/profiler.ts:117,168` CPU/heap
- `packages/coding-agent/src/debug/remote-debugger.ts:105` JSC-инспектор
- `packages/coding-agent/src/debug/system-info.ts` снимок системы
- `packages/coding-agent/src/session/agent-session.ts:1154,1919,1927,1942,1949` SSE-буфер
- `packages/ai/src/utils/sse-debug.ts:6` `notifyRawSseEvent`
- `packages/ai/src/utils/request-debug.ts:4` `PI_REQ_DEBUG`
- `packages/ai/src/utils/http-inspector.ts:29` `SENSITIVE_HEADER_PATTERN`
- `packages/tui/src/apps/debug/raw-sse-buffer.ts:4-6,199-227,254,383` ring и триммер
- `packages/tui/src/apps/debug/log-viewer.ts:29-36` чанки и кэп; `:25` маркер границы сессии
- `packages/tui/src/apps/debug/viewer-frame.ts` chrome вьюеров
- `packages/tui/src/tools/debug.ts:68-95,185-244` рендерер
- `packages/tui/src/tui.ts:2952` `Shift+Ctrl+D`
- `packages/coding-agent/src/slash-commands/builtin-lifecycle.ts:522` `/debug`
- `packages/coding-agent/src/tools/settings.ts:545-553` `debug.enabled`
- `packages/coding-agent/src/activity/index.ts:208` `AgentActivityIndex`
- `packages/coding-agent/src/stream/recording.ts:97` `.ompcast`
- `packages/coding-agent/src/modes/controllers/command-controller.ts:452` transcript dump

**omp, time-travel и хуки**
- `packages/coding-agent/src/tools/checkpoint.ts:53,89` checkpoint/rewind
- `packages/coding-agent/src/session/agent-session.ts:10507,10550` rewind-энфорс и `#applyRewind`
- `packages/coding-agent/src/extensibility/shared-events.ts:325-352` `ToolCallEventResult` с `input`
- `packages/agent/src/agent.ts:1150,1154,1167,1176,1195` интерсепторы
- `packages/agent/src/types.ts:1253-1275` `AgentEvent`

**наш форк**
- `crates/pa-agent/src/types.rs:583-621,629-645` `AgentEvent`, `BeforeToolCallResult`
- `crates/pa-agent/src/agent.rs:96-97` `before_tool_call` / `after_tool_call`
- `crates/pa-agent/src/agent_loop/mod.rs:76` точка применения хука
- `crates/pa-lsp/src/client.rs:289` `LspClient::spawn`; `crates/pa-lsp/Cargo.toml:23-27` `async-lsp`
- `crates/pa-core/src/session_engine/engine.rs:537` регистрация LSP-тула
- `crates/pa-core/src/session_engine/side_question.rs:177` пример `before_tool_call`
- `crates/pa-daemon/src/debug_log.rs:47-115,227` ротация логов
- `crates/pa-tui/src/info_commands.rs:182` `logs_rows`; `info_commands/cache.rs:124` `cache_rows`
- `crates/pa-tui/src/session_ui/commands.rs:641,709` `/cache`, `/logs`
- `crates/pa-tui/src/info_panel.rs:25-110` `InfoContent` / `InfoPanel`
- `crates/pa-types/src/daemon/command.rs:246,645,655` `Attach`, `GetContextTree`, `GetCacheStats`
- `crates/pa-types/src/daemon/mod.rs:67` `DaemonReplayInfo`
- `crates/pa-daemon/src/engine/scripted.rs:28` `ScriptedEngine`
- `crates/pa-core/src/tools/golden_replay.rs:26` `CORPUS_DIR`
- `crates/pa-core/src/session/manager_ext.rs:199,221` `branch` / `branch_with_summary`
- `crates/pa-core/src/session/manager/append.rs:354` `set_leaf_id`
