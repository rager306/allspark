# Black-box QA кодинг-харнеса: как тестируют шесть систем 2025-2026 и что делать нам

**Дата:** 2026-10-19 · **Режим:** read-only разбор + эмпирическая проверка на нашем бинаре
**Объекты:** OpenAI Codex 0.163.0, oh-my-pi (omp), xai-grok (Grok Build), Command Code 1.84.0, sst/opencode (dev), gsd-pi 1.22.0
**Наш объект:** форк prime-agent (Rust, workspace `crates/pa-*`), бинарь `target/debug/prime-agent`

Все находки — с указанием `файл:строка`. Локальные пути:

| Система | Локальный корень | Чем располагаем |
|---|---|---|
| Codex | `/usr/lib/node_modules/@openai/codex` (только собранный бинарь) + `gh api` дерево `openai/codex` (10 282 пути) | исходники тестов через GitHub raw |
| oh-my-pi | `/tmp/omp-src` (TS-монорепо, 3 109 `*.test.ts`) | полные исходники |
| Grok Build | `/tmp/grok-build` (Rust, 110M) | полные исходники, включая PTY-харнесс |
| Command Code | `/usr/lib/node_modules/command-code` (бандл 3.3M, `dist/*.mjs`) | только бандл + docs-скиллы |
| opencode | `/tmp/opencode-src` (Go-форк, 4 теста) + `gh api` `sst/opencode@dev` (9 502 пути) | исходники тестов через GitHub raw |
| gsd-pi | `/usr/lib/node_modules/@opengsd/gsd-pi` (dist) + `gh api` `open-gsd/gsd-pi` | `tests/e2e/*` через GitHub raw |

---

## 0. TL;DR — пять выводов, которые меняют дизайн

1. **Никто не проверяет «задачу» через LLM-судью в CI-гейте.** Все шесть систем держат блокирующий гейт на детерминированных оракулах (состояние ФС, exit code, типы/счётчики событий), а судью/Live-модель выносят в отдельный, не-блокирующий тир. Anthropic-канон «grade what the agent produced, not the path it took» реализован буквально: **состояние среды + инварианты трейса**, не текст ответа.
2. **Стабильность даёт не retry, а scripted-провайдер.** У всех, у кого есть воспроизводимый e2e, есть свой «фейковый провайдер» с ожиданиями по форме запроса: Codex — `wiremock` + SSE-скрипты, Grok — `MockInferenceServer`, omp — `createMockModel`, gsd — `GSD_FAKE_LLM_TRANSCRIPT` (JSONL), opencode — `SimulatedProvider`/кассеты. Retry — только страховка (Codex: `retries = 1`).
3. **Формат ответа модели — тоже контракт.** gsd-фейк провайдер падает, если `messageCount`/`toolNames`/`hasToolResultFor` не совпали (`fake.js:65`). Это ловит дрейф промпта, который обычные unit-тесты не видят.
4. **PTY нужен только для TUI-контракта.** Grok — единственный, кто делает это всерьёз (176 файлов PTY-тестов, декларативные YAML-сценарии, запись asciinema). Для проверки *кодинг-задачи* PTY не нужен: `--mode json` даёт NDJSON-поток событий и дешевле на порядок.
5. **У нас уже есть 80% инфраструктуры.** `qa-tasks/` + `scripts/qa/run_suite.py` (1 957 строк) — это уровень 1–2 из канона, с манифестами, канарейкой и delta-гейтом. Не хватает ровно одного: **black-box теста «типовая кодинг-задача → диффы+трейс+стоимость» с жёсткими инвариантами и нулевой флакостью**. Дизайн — в §5, он проверен эмпирически (§5.7).

---

## 1. Как каждая система запускает harness в тесте (A)

### 1.1 OpenAI Codex — in-process + `wiremock`, бинарь только для CLI-границ

| Что | Как | Источник |
|---|---|---|
| Основной e2e | `wiremock::MockServer` на loopback + реальный `codex_core::ThreadManager` в процессе теста | `codex-rs/core/tests/common/responses.rs:1205` (`start_mock_server`) |
| Провайдер | SSE-скрипты: `mount_sse_once` / `mount_sse_sequence` (паника, если запросов больше, чем тел) | `responses.rs:1146`, `:1471` |
| Изоляция состояния | `codex_home` = `tempfile::TempDir` на каждый тест; комментарий прямо про «не засорять реальный `~/.codex`» | `core/tests/common/lib.rs:197-200` |
| Конфиг | `ConfigBuilder::loader_overrides(LoaderOverrides::without_managed_config_for_tests())` — managed-конфиг выключен | `lib.rs:216-222` |
| Песочница | `codex_linux_sandbox_exe` подставляется из `cargo_bin`, `CODEX_TEST_ENVIRONMENT=local|docker|wine-exec` выбирает исполнителя | `lib.rs:238-246`, `test_codex.rs:617` |
| Детерминизм PID | `ctor` в тест-процессе включает детерминированные id процессов | `lib.rs:62-66` |
| Реальный бинарь | Только для CLI-границ: `codex_bin` / `cargo_bin("codex")`; apply_patch-сценарии гоняют бинарь `apply_patch` в tempdir | `apply-patch/tests/suite/scenarios.rs:1-60`, `cli/e2e_benches/codex_help.rs:14` |
| Изоляция трейсов | Тест перезапускает **сам себя** как subprocess (`CODEX_TURN_PHASE_TRACE_TEST_SUBPROCESS=1`), чтобы OTel-коллектор был свой | `core/tests/suite/turn_phase_trace.rs:41-70` |

Ключевой приём: **два уровня** — in-process ядро (быстро, сотни тестов) и subprocess только там, где контракт процессный (argv, сигналы, OTel-глобали, sandbox).

### 1.2 Grok Build — реальный бинарь в PTY + мок-инференс + sandbox `$HOME`

| Что | Как | Источник |
|---|---|---|
| Бинарь | `PAGER_BINARY` → `CARGO_BIN_EXE_xai-grok-pager` → сам собирает `cargo build -p xai-grok-pager-bin` | `src/env.rs:56-80` |
| PTY | `portable-pty`; на Unix — свой fork/exec с `setsid` + controlling TTY + `pdeathsig` | `src/pty_spawn.rs:1-4`, `src/pty.rs:96-200` |
| Терминал-эмулятор | `alacritty_terminal` через `ptyctl`: экран, стили, HTML, SVG, ответы на DSR/DA1 | `src/screen.rs:1-20` |
| Мок-инференс | `MockInferenceServer` (loopback, случайный порт) + `TestSandbox` с изолированным `$HOME` | `src/content.rs:102-131` |
| Ответы | `ScriptedResponse::sse(...)` — точный SSE-скрипт; `expect_agent_turn` регистрирует **оба** бэкенда (Responses + Chat Completions) | `src/content.rs:194-267` |
| Изоляция env | Явные списки: `APPEARANCE_ENV_VARS` (тема/цвет), `HOST_TERMINAL_ENV_VARS` (бренд терминала, tmux, nvim, Cursor) — всё вычищается, иначе ребёнок классифицируется иначе | `src/pty_spawn.rs:19-70` |
| Env-ops | Типизированные `EnvOp::set` / `EnvOp::remove` (не «просто переопределить») | `src/pty.rs:43-62` |
| Размер терминала | Фикс 50×120 по умолчанию; минимальный режим требует `set_respond_to_queries(true)`, иначе молча деградирует | `tests/pty_e2e/common.rs:16-18`, `:1184-1200` |

### 1.3 oh-my-pi — три уровня: чистый `VirtualTerminal`, реальный `Bun.Terminal`, in-process сессия

| Что | Как | Источник |
|---|---|---|
| TUI-уровень | `VirtualTerminal` на **настоящем ядре kitty** (`kitty-vt-wasm`, screen.c + vt-parser.c): графемы, wide-char, BCE, scrollback — «ground truth modern-terminal semantics» | `packages/tui/test/virtual-terminal.ts:1-40` |
| Реальный PTY | `Bun.Terminal` + `@oh-my-pi/pi-utils/vterm`, `Bun.spawn([...], {terminal})`; фикстура — отдельный скрипт `fixtures/fatal-tui.ts` | `packages/coding-agent/test/fatal-stderr-pty.test.ts:12-46` |
| Полный e2e сессии | `InteractiveMode` + `AgentSession` in-process, модель-мок, `waitForRender(predicate)` | `packages/coding-agent/test/interactive-terminal-e2e.test.ts:30-80` |
| Мок-провайдер | `createMockModel({responses|handler|asyncGenerator})`, `.calls` для проверок; `registerMockApi()` для глобального `stream()` | `packages/ai/src/providers/mock.ts:244-272`, шапка файла `:1-45` |
| Env-изоляция | `TempDir.createSync("@pi-...")` + `Settings.init({inMemory:true})` + `SessionManager.inMemory()` + `AuthStorage.create(":memory:")` | `interactive-terminal-e2e.test.ts:36-52` |
| Прогон | `bun test` чанками по 5–10 файлов (иначе SIGKILL 137 по OOM), `--parallel` по бакетам | `scripts/ci-test-ts.ts:64-82` |

### 1.4 Command Code — тестов нет в пакете, есть **контракт headless**

Пакет `command-code@1.84.0` содержит только `dist/*.mjs` + `vsix`; тестовых файлов нет (`find . -name "*test*"` → пусто). Но есть подробно описанный машинный контракт (`dist/bundled/command-code-knowledge/reference/headless.md`), который и есть спека для black-box теста:

- `-p/--print`, stdin-pipe, `--output-format text|json`;
- NDJSON: event-фреймы `{"type":"event","event":{"type":"tool_running",...}}` + **ровно одна финальная строка** `{"type":"result","subtype":"success|error|max_turns","usage":{...},"durationMs":...,"finalText":...}` (`headless.md:109-140`);
- `sessionId`/`stopReason` **опциональны** — «скрипт, который индексирует их безусловно, сломается именно на тех кейсах, где они нужны» (`headless.md:138-140`);
- таксономия exit-кодов 0/1/3/4/5/6/7/8/9/10/130 (`headless.md:192-206`);
- `--max-turns` (дефолт 100, код 8 при исчерпании), `--verbose` печатает session id **в stderr**, чтобы stdout остался чистым (`headless.md:166-170`).

**Урок:** когда исходников нет, чужой публичный контракт — готовый набор ассертов для нашего differential-теста.

### 1.5 opencode — «сервер как процесс» + мок API на уровне страницы/сокета

| Что | Как | Источник |
|---|---|---|
| App e2e | Playwright + `mockOpenCodeServer(page, config)`: `page.route("**/api/**")` через Effect HttpApi, фейковый `/api/event` SSE, PTY через `routeWebSocket` | `packages/app/e2e/utils/mock-server.ts:202-230` |
| Жёсткость мока | Любой не-замоканный маршрут → **501 + `Unmocked API request`** и падение теста | `mock-server.ts:410` |
| CLI/ACP | Реальный процесс + `Bun.serve` на `port: 0` как LLM; JSON-RPC по stdio | `packages/cli/test/acp/subprocess.ts:44-90` |
| Env-изоляция | `isolatedEnv()`: `HOME`, `XDG_{CACHE,CONFIG,DATA,STATE}_HOME`, `OPENCODE_DB`, `OPENCODE_CONFIG_DIR`, отключение filewatcher/автообновления; **общий** транспайлер-кэш, чтобы не платить за него в каждом процессе | `packages/cli/test/fixture/environment.ts:1-26` |
| TUI | `testRender`/`createAppFixture` (OpenTUI), `kittyKeyboard: true`, `mockInput`, `ManualClock` | `.opencode/skills/tui-tests/SKILL.md:34-37` |
| Property | `fast-check` на протокол PTY-сокета: сценарии шагов с весами + латентности (yields vs millis) | `packages/server/test/pty-socket.property.test.ts:1-60` |
| CI | `bun turbo test --affected --concurrency=3`, e2e — 2 шарда на ОС, `timeout-minutes: 15` | `.github/workflows/test.yml:99-107`, `:274-279` |

### 1.6 gsd-pi — реальный бинарь + JSONL-транскрипт фейк-LLM

| Что | Как | Источник |
|---|---|---|
| Бинарь | `GSD_SMOKE_BINARY` → `node <path>` иначе `gsd` из PATH | `tests/e2e/_shared/spawn.ts:120-131` |
| Env | `buildE2eEnv`: вычищает **все** `GSD_*` (кроме `GSD_NATIVE_PREFER_LOCAL`), `GSD_NON_INTERACTIVE=1`, `TMPDIR` = realpath (macOS `/var` vs `/private/var`), `HOME` = per-process изолированный | `spawn.ts:74-100` |
| Фейк-LLM | `GSD_FAKE_LLM_TRANSCRIPT=<jsonl>`; провайдер читает транскрипт, по одному турну на вызов | `packages/pi-ai/dist/providers/fake.js:1-30`, `:158` |
| Ожидания по форме | `modelId`, `messageCount`, `lastUserText` (substring), `systemContains[]`, `toolNames` (точный набор), `hasToolResultFor` | `fake.js:65-110` |
| Эмиты | `text`, `tool_use`, `error_429` (+`retryAfterMs`), `malformed`, `timeout` (`delayMs`) | `fake.js:204+`, типы в `tests/e2e/_shared/fake-llm.ts:26-35` |
| Артефакты | `artifactsFor(slug)` → каталог на тест, `E2E_ARTIFACTS_DIR`, выгрузка в CI при падении | `tests/e2e/_shared/artifacts.ts:1-40` |
| Проект | `createTmpProject({git:true})` — `git init --initial-branch=main` + фикс identity, «иначе inconsistent default branch» | `tmp-project.ts:44-62` |

---

## 2. Как подают задачу (B)

| Система | Промпт | Фикстура | Размер |
|---|---|---|---|
| Codex | Литерал в тесте (`submit_turn("first")`), либо динамические tools | `TempDir` + файлы, созданные кодом теста | Микро: 1–3 турна, файл на 1–5 строк (`apply_patch` сценарии — `input/` + `patch.txt`) |
| Grok | `PROMPT = "go"` (2 символа!) + сентинел ответа `MOCKRESPONSE` | `WorkspaceConfig{git_init, files}` в YAML-сценарии; `merge_fix.py` на 28 строк для edit-теста | Микро, но сценариев 45 YAML + 176 Rust-тестов |
| omp | Реальный промпт в `mode.getUserInput()`; бенчмарк — `prompt.md` на задачу | `fixtures.tar.gz`: `input/` + `expected/` + `metadata.json` (seed, mutationType, difficulty) | Средний: 225 задач в стиле Exercism, мутации реального TS |
| Command Code | `cmd -p "..."` | нет (внешний) | — |
| opencode | Строки в тесте (`"hi there omp"`, `MARKER_DRAFT`) | Мок-объекты в памяти (`mockWorkspace`), `C:/OpenCode/<name>` на любой ОС — «ничего не трогает диск» | Микро |
| gsd | `runWithFakeLlm(transcript, {prompt:"use a tool"})` | `createTmpProject({files})` + скелет `.gsd/`; в `headless-auto-pause-blocked` — реальный мини-проект с `src/answer.js` + `test/answer.test.js` + git-коммиты | Малый, но с git-историей |

**Вывод:** для «типовой кодинг-задачи» все берут **маленький синтетический репозиторий** (5–60 строк), а не клон реального проекта. Реальные проекты (SWE-bench-стиль) — только в отдельных бенчмарк-тирах, никогда в блокирующем CI.

---

## 3. Как проверяют результат (C)

### 3.1 Иерархия оракулов (по возрастанию хрупкости)

```
1. Состояние ФС (diff/snapshot каталога)      ← самый частый, самый надёжный
2. Exit code / форма stdout (NDJSON-схема)
3. Типы и счётчики событий трейса (не порядок целиком)
4. Структурные ожидания запроса к модели
5. Snapshot отрендеренного экрана (insta)
6. Тайминги (только с широким бюджетом и barrier-синхронизацией)
7. LLM-judge (никогда в блокирующем гейте)
```

| Приём | Кто | Источник |
|---|---|---|
| **Snapshot каталога**: `BTreeMap<PathBuf, Entry>` из `expected/` vs фактического — побайтово | Codex | `apply-patch/tests/suite/scenarios.rs:63-104` |
| **Точный diff-стат в выводе инструмента**: `"diffs":[{"path","diff":"-2\|...\n+2\|..."}]` | Codex (аналог), у нас так же | проверено эмпирически (§5.7) |
| **Формат-эквивалентность** вместо байтового равенства + `indentScore` (насколько форматтер «спасал» агента) | omp | `typescript-edit-benchmark/src/verify.ts:14-25`, `:143-175` |
| **Токен-учёт как данные**: записи `TokenUsageRecord` в rollout, `turn=120, thread=120 → 200 → 230` | Codex | `core/tests/suite/token_usage_rollout.rs:80-110` |
| **Структурные ожидания запроса** (messageCount/toolNames/hasToolResultFor) | gsd | `providers/fake.js:65-110` |
| **Типы событий жизненного цикла** (`tool_execution_start` + `tool_execution_end` оба есть) | gsd | `tests/e2e/agent-loop.e2e.test.ts:118-130` |
| **Точный порядок событий** (только для in-process ядра, не для CLI) | omp/наш | `crates/pa-agent/tests/agent_loop_scripted.rs:250-270` |
| **Insta-snapshot** отрендеренного markdown/стрима, 1 433 `.snap` в `codex-rs/tui` | Codex | `tui/src/streaming/render_tests.rs:110-140` |
| **Инвариант «инкрементальный рендер == полный рендер»** после каждого чанка | Codex | `render_tests.rs:47-60` (`assert_eq!(render.lines, render_source(source,...))`) |
| **PTY-скриншот + OCR-подобные проверки** (текст на экране, отсутствие текста, стили/фон) | Grok | `src/lib.rs:447-476`, `src/scripted.rs:204-368` |
| **Инварианты по логу продюсера** (не по часам теста) | Grok | `src/scroll_matrix/invariants.rs:1-10` |
| **Метрики каталога в манифесте**: `tool_share`, `median_tool_turns`, `p95_duration_s` | мы | `scripts/qa/run_suite.py:1667` |

### 3.2 Три «золотых» правила из практики

**(a) Нормализация волатильного до сравнения.** Codex пишет целый `Normalizer`: рабочие каталоги → `<WORKING_DIRECTORY n>`, UUID → `<UUID n>`, prompt-cache ключи → `<PROMPT_CACHE_KEY n>`, известные сегменты промпта (`PERMISSIONS_INSTRUCTIONS`, `ENVIRONMENT_CONTEXT`) → маркеры (`core/tests/common/context_snapshot/normalize.rs:1-120`). Без этого любой snapshot промпта умирает на первом же прогоне.

**(b) Проверять «должно» И «не должно».** У нас в `qa-tasks/RT-008` это уже есть: `must_not: {tool: bash, args_regex: printenv QA_DEPLOY_TOKEN}` + `output: value-never-printed`. У gsd — `assertNoToolErrors()`/`assertNoOperatorFailures()` как обязательный шаг каждого workflow-теста (`tests/e2e/_shared/workflow-scenario.ts:60-80`).

**(c) Стоимость — first-class.** Codex проверяет, что usage **накопительный и персистентный** между резами сессии. У Command Code `usage` — обязательное поле финального `result`-фрейма. У нас usage уже в `turn_end.message.usage` (проверено: `totalTokens` побитово стабилен между прогонами, §5.7).

---

## 4. Как стабилизируют (D) — каталог приёмов

### 4.1 Мок/скрипт провайдера (главный приём)

| Система | Механизм | Источник |
|---|---|---|
| Codex | `wiremock` + `mount_sse_once/sequence`; последовательность паникует при лишнем запросе | `responses.rs:1471-1500` |
| Grok | `MockInferenceServer` + `expect_agent_turn` (оба API), `set_chunk_delay` для «висящего» стрима | `src/content.rs:169-171`, `:194` |
| omp | `createMockModel({responses \| handler \| asyncGenerator})`; async-генератор умеет ждать внешнего сигнала между турнами | `packages/ai/src/providers/mock.ts:1-45` |
| gsd | JSONL-транскрипт + `checkExpectations` (падает на дрейфе) | `providers/fake.js:65-110` |
| opencode | `SimulatedProvider` (JSON-RPC «Drive» управляет llm.request/chunk/finish) + кассеты | `packages/simulation/test/simulated-provider.test.ts:1-60` |
| мы | `PRIME_AGENT_FAUX_SCRIPT` (`{responses:[...]}`), грамматика `text`/`thinking`/`toolCall` | `crates/pa-ai/src/providers/faux/script.rs:26-45`, `:175-200` |

### 4.2 VCR / кассеты (запись реального трафика один раз)

opencode `@opencode/http-recorder` — самый зрелый пример:
- `HttpRecorder.layer(name)` / `layerFetch` / `layerSocket` / `layerWebSocketConstructor`;
- локально: кассеты нет → записать; **`CI=true` → кассета отсутствует = FAIL** (не «тихо записать»);
- редактирование: заголовки/URL/JSON-поля (`authorization`, `set-cookie`, `x-api-key`, `access_token`, `client_secret`, `password`…) → `[REDACTED]` (`packages/http-recorder/src/redaction/redactor.ts:8-30`, `:156`);
- **отказ записывать кассету с секретом**: `UnsafeCassetteError` → запрос падает, файл не создаётся (`test/cassette.test.ts:12-30`);
- WebSocket-кассеты сохраняют **порядок** клиент/сервер фреймов и при replay ждут, пока приложение напишет совпадающий клиентский фрейм (детерминизм без воспроизведения таймингов) (`README.md:144-152`).

### 4.3 Ожидание состояния, а не времени

| Приём | Источник |
|---|---|
`wait_until(desc, timeout, predicate)` с диагностикой (дерево процессов + экран + raw) в сообщении об ошибке | `xai-grok.../src/lib.rs:365-398`
`wait_until_stable(desc, timeout, hold, predicate)` — условие должно **удержаться** `hold` (для «турн завершился») | `src/lib.rs:403-444`
`wait_for_turn_idle` = stable + «нет `Ctrl+c:cancel` / `Waiting for response`» | `src/lib.rs:465-476`
`wait_for_event_with_timeout` — таймаут не меньше 10s «на async startup (config IO, tool discovery)» | `codex core/tests/common/lib.rs:381-401`
`waitForRender(predicate)` — кадровый барьер | `omp virtual-terminal.ts:232`
`waitForFrame` с `maxPasses` **и предупреждением «не используй maxPasses как часы»** | `opencode .opencode/skills/tui-tests/SKILL.md:14-25`
`waitFor` с предикатом на **фокус** вместо бюджета кадров (иначе ключи теряются, а не опаздывают) | там же `:18`
`transport.send` резолвится по доставке; `holdRoute(...).arrived` | `opencode app-e2e SKILL.md:70-72`

### 4.4 Ретраи и таймауты — только как страховка

| Приём | Источник |
|---|---|
`retries = 1` + `slow-timeout = {period:"30s", terminate-after:2}` (убить после 60s) | `codex-rs/.config/nextest.toml:5-6`
`test-groups` с `max-threads = 1` для process-heavy наборов (app-server, apply_patch CLI) | `nextest.toml:33-46`
`threads-required = "num-test-threads"` для тестов, которым нужен весь раннер | `nextest.toml:20-24`
PTY-наборы помечены `#[ignore]` и включаются только Bazel'ом с лимитом 4 воркера | `tests/pty_e2e_smoke.rs:1-5`
`RUN_LOCK: Mutex<()>` / `SERIAL: tokio::sync::Mutex` для тестов с глобальным состоянием (мышь, сигналы) | `pa-tui/tests/drag_select_perf.rs:30-40`, `xai-grok tests/scroll_matrix_curated.rs:19`
`HARNESS_LOCK` — «concurrent process-group signals and raw ptys flake on shared CPUs» | `pa-cli/tests/kitty_early_typing_e2e.rs:75-85`

### 4.5 Детерминизм времени

| Приём | Источник |
|---|---|
`ManualClock` + `renderer.pause()` + `renderOnce()` — кадры двигает тест | `opencode tui SKILL.md:34`
Effect `TestClock` + `advance(condition)`/`drain` (debounce регистрируется через fiber hops) | `opencode unit-tests SKILL.md:22-40`
`jest.useFakeTimers()` только если нечего инжектить, с восстановлением в `finally` | там же `:35`
**Наш `tokio` `test-util` (paused clock)** уже в dev-deps `pa-cli` | `crates/pa-cli/Cargo.toml:44-46`
Инварианты читают **часы продюсера** (`ts_ms`, `ms_since_prev_flush`), а не часы теста — «CI-нагрузка может только растянуть, поэтому false-fail невозможен» | `xai-grok src/scroll_matrix/invariants.rs:1-10`

### 4.6 Процессная гигиена (утечки, зомби, порты)

| Приём | Источник |
|---|---|
`Drop` для `PtyController`: SIGTERM → grace 500ms → SIGKILL дерева, reap ≤250ms, `kill_tree_best_effort` | `xai-grok src/pty.rs:13-18`, `:410-470`
`quit_with_double_ctrl_c` + `wait_exit_code(15s)` с fallback на kill | `tests/pty_e2e/common.rs:1514-1523`
`kill(graceMs=2000)`: SIGTERM → ждать → SIGKILL → ждать `exitPromise` | `gsd spawn.ts:200-215`
`port: 0` всегда, «никогда не хардкодь порт: Windows выдаёт 49152+, он может быть занят» | `opencode tui SKILL.md:37`
orphan-reap фикстура: отдельный бинарь держит ребёнка, тест убивает родителя и проверяет, что ребёнок умер | `xai-grok src/bin/pty_orphan_holder.rs`, `tests/orphan_reap.rs`
`t.after(cleanup)` вместо `try/finally` — правило проекта | `gsd tests/e2e/README.md:41`
`rmSync(dir,{recursive:true,force:true,maxRetries:20,retryDelay:100})` — Windows file-lock | `opencode cli/test/acp/subprocess.ts:100-110`

### 4.7 Канарейка секретов на весь сьют

- gsd: `buildE2eEnv` вычищает все `GSD_*`, поэтому локальный конфиг не протекает (`spawn.ts:76-84`).
- мы: **случайный `QA_SUITE_CANARY` в каждом триале**, грепается в транскрипте и stdout/stderr — инвариант «нет утечки секрета» проверяется не только в RT-008 (`run_suite.py:935`, `qa_logs.py:628`).
- opencode: redactor отказывается писать кассету с похожим на секрет телом.

### 4.8 Артефакты на падении

| Система | Что пишется |
|---|---|
Grok | `report.json` + `bugs.md` + `<step>.{txt,html,svg,json}` + `raw_output.bin` + опционально asciinema-cast (`GROK_PTY_CAST_DIR`) — `src/scripted.rs:1031-1063`, `:637`, `common.rs:1557`
gsd | `artifactsFor(slug)` → каталог на тест, CI-upload (`_shared/artifacts.ts`)
Codex | `body_print_limit(80_000)` — чтобы в отчёте был реальный запрос, а не «body too large» (`responses.rs:1207`)
мы | `timeline.json`, `telemetry.json`, `session.jsonl`, `product-*.log` с редактированием секретов (`run_suite.py:1600-1625`)

---

## 5. Что проверяют помимо «задача решена» (E)

| Проверка | Кто и как | Источник |
|---|---|---|
**Отсутствие лишних правок** | snapshot каталога целиком (любой новый файл = провал) | `codex apply-patch scenarios.rs:80-104` |
**Отсутствие временных файлов** | `AssertNoTempArtifacts` — сканирует `$HOME` на `*.tmp` | `xai-grok src/scripted.rs:1519` |
**Порядок/наличие tool-вызовов** | `must: {tool, args_regex}`, `must_not`, `max_repeat_calls` | `qa-tasks/README.md` (грамматика) |
**Tool-ошибки = провал** | `assertNoToolErrors()` | `gsd workflow-scenario.ts:66-70` |
**Число запросов к провайдеру (бюджет ретраев)** | `max_requests`, `max_consecutive_failed_requests` | `qa_logs.py:548-582` |
**Токены и стоимость** | `TokenUsageRecord` (накопление turn/thread), `usage` в result-фрейме | `token_usage_rollout.rs`, `headless.md:128-134` |
**Скорость фазы** | `prompt-built`/`first-byte`/`first-token`/`stream-done` в `logs/agent.jsonl` | наш `request_timing.rs:30`, проверено §5.7 |
**Параллелизм инструментов** | barrier-синхронизация + бюджет 1 600ms | `codex tool_parallelism.rs:84-90` |
**Парсимость/форма ответа** | NDJSON-схема, `subtype` первым полем | `headless.md:117-140` |
**Изоляция (env не протёк)** | hostile env + grep | `qa-tasks/RT-007`, `RT-008` |
**Схема транскрипта (drift)** | `transcript_schema` + `compare_schemas` (sha256 набора полей) | `qa_logs.py:290-334` |
**Регрессия против baseline** | pass-rate drop ≥3pp или ≥2 stable-задач потеряли pass-all → exit 1 | `run_suite.py:1707` |
**Поведенческий фингерпринт** | `{tool_share, median_tool_turns, p95_duration_s, cancel_share, subagent_share}` | `run_suite.py:1667` |

---

## 6. Группировка по темам: что тестируют и чем

| Тир | Что проверяет | Инструмент | Кто эталонно делает | Время |
|---|---|---|---|---|
**unit** | чистая логика: парсеры, схемы, редьюсеры | `cargo test --lib` / `bun test` | все; Codex: 253 файла в `core/tests/suite`, omp: 3 109 тестов | сек–мин |
**property** | инварианты протокола при произвольных интерливингах | `fast-check`, `proptest` | opencode `pty-socket.property.test.ts`; у нас `proptest` в `pa-tui/geometry.rs`, `pa-daemon/supervisor/tests.rs` | мин |
**integration (in-process)** | ядро + мок-провайдер: цикл турна, tool-результаты, компакция | `wiremock` / `createMockModel` / faux | Codex, omp, мы (`pa-agent/tests/agent_loop_scripted.rs`) | сек |
**snapshot/regression** | рендер, промпт-сборка, схемы | `insta` (1 433 `.snap`), `toMatchSnapshot` | Codex | мс–сек |
**CLI-contract (black-box, headless)** | argv, exit code, NDJSON-схема, изоляция env | spawn бинаря + JSON-парсер | gsd (`tests/e2e/*.e2e.test.ts`), Command Code (контракт) | 0.1–3 с |
**eval / benchmark** | «задача решена», edit-качество | фикстуры `input/expected`, формат-эквивалентность | omp `typescript-edit-benchmark` | мин |
**PTY e2e** | TUI-контракт: клавиши, экран, mouse, resize | `portable-pty` + VT-эмулятор | Grok (176 файлов), omp (`Bun.Terminal`) | сек |
**perf/bench** | p99 кадра, boot-медиана, drag-cost | `divan`, `hyperfine`, `FrameTiming` | Grok (`pty_baselines`), omp (`bench-guard.ts`), мы (`pa-perf`) | мин |
**smoke** | «бинарь вообще стартует и отвечает» | 3–5 тестов | gsd `sanity.e2e.test.ts`, Grok `pty_e2e_smoke` | сек |
**differential** | наш vs эталон (TS/старая версия) | два бинаря на одном входе | мы (`crates/pa-cli/tests/differential_cli.rs`, `export_live_differential.rs`) | мин |
**live/real-model** | качество на живой модели | реальные креды, opt-in | gsd `tests/live*`, мы `--lane real-model` | мин–десятки мин |

---

## 7. Каталог фишек (конкретные приёмы с источником)

| # | Фишка | Зачем | Источник |
|---|---|---|---|
1 | Мок-сервер падает на **лишнем** запросе | ловит лишние ретраи/петли | `codex responses.rs:1471-1500` |
2 | Мок отвечает **501 + текст** на незамоканный маршрут | ловит «тихо прошло mimo мока» | `opencode mock-server.ts:410` |
3 | Скрипт провайдера **валидирует форму запроса** (messageCount, toolNames, hasToolResultFor) | ловит дрейф промпта | `gsd fake.js:65-110` |
4 | Эмит `error_429` / `malformed` / `timeout` из фейка | тест error-путей без сети | `gsd fake-llm.ts:26-35` |
5 | Async-генератор ответов с внешним `await` между турнами | детерминированная гонка «пока турн идёт» | `omp mock.ts:21-28` |
6 | `set_chunk_delay` — растянуть стрим | тесты отмены/очереди во время стрима | `xai-grok content.rs:169` |
7 | `expect_agent_turn` на **оба** API сразу | тест не зависит от того, какой бэкенд выбран | `xai-grok content.rs:194-267` |
8 | Barrier-синхронизация вместо тайминга для «параллельно» | нет флейков на медленном CI | `codex tool_parallelism.rs:84-90` |
9 | `wait_until_stable(hold)` для «турн завершился» | не ловит промежуточный кадр | `xai-grok lib.rs:403` |
10 | Диагностика в тексте таймаута: дерево процессов + экран + raw | падение читается без ре-рана | `xai-grok lib.rs:378-396` |
11 | Нормализатор волатильного (UUID, cwd, cache-key) | snapshot не умирает | `codex normalize.rs:1-120` |
12 | Инкрементальный рендер == полный (после каждого чанка) | ловит streaming-баги | `codex render_tests.rs:47-60` |
13 | Snapshot каталога `expected/` побайтово | «лишняя правка» = провал | `codex apply-patch scenarios.rs:80-104` |
14 | Формат-эквивалентность + `indentScore` | не наказывает за пробелы, но меряет качество | `omp verify.ts:143-175` |
15 | `GSD_*`-стрип + изолированный `HOME` + realpath `TMPDIR` | env-протечки и macOS-симлинки | `gsd spawn.ts:74-100` |
16 | Канарейка-секрет на весь сьют | утечка ловится везде | наш `run_suite.py:935` |
17 | Redactor, **отказывающийся** писать кассету с секретом | кассеты не станут утечкой | `opencode cassette.test.ts:12-30` |
18 | `CI=true` + нет кассеты = FAIL | нельзя «тихо записать» в CI | `opencode http-recorder README.md:69` |
19 | `#[ignore]` + отдельный раннер с лимитом воркеров для PTY | PTY не тормозит обычный прогон | `xai-grok Cargo.toml` + `pty_e2e_smoke.rs` |
20 | Пер-платформенные baseline'ы p99 + порог 15% | перф-гейт без ложных срабатываний | `xai-grok benches/pty_baselines/README.md` |
21 | `bench-guard.ts` — медиана hyperfine +5%, **локально, не в CI** («boot wall-clock machine-relative») | честный перф-гейт | `omp scripts/bench-guard.ts:1-20` |
22 | Инварианты читают часы продюсера | нет false-fail под нагрузкой | `xai-grok scroll_matrix/invariants.rs:1-10` |
23 | asciinema-cast по env-переменной | воспроизводимое доказательство для багрепорта | `xai-grok common.rs:1557` |
24 | `#[ctor]` включает детерминированные id процессов в тест-процессе | стабильные snapshot'ы | `codex common/lib.rs:62-66` |
25 | Само-перезапуск теста как subprocess для глобального коллектора | изоляция OTel | `codex turn_phase_trace.rs:41-70` |
26 | `nextest` test-groups с `max-threads=1` | process-heavy не флейкуют | `codex nextest.toml:33-46` |
27 | Чанкинг тест-файлов против OOM (SIGKILL 137) | большой монорепо тестируется | `omp ci-test-ts.ts:64-82` |
28 | `artifactsFor()` на тест + CI-upload | пост-мортем без ре-рана | `gsd artifacts.ts` |
29 | Таксономия exit-кодов как контракт | CI различает «нет кредов» и «баг» | `command-code headless.md:192-206` |
30 | `usage`/`durationMs` в финальном result-фрейме | стоимость и время — часть контракта | `command-code headless.md:128-134` |
31 | Манифест прогона: `suiteRev` (content hash + git head) + `binaryBuild` | «прогон без манифеста — не доказательство» | наш `run_suite.py:1687` |
32 | Delta-гейт: pass-rate −3pp или 2 stable-задачи | ловит тихую деградацию | наш `run_suite.py:1707` |
33 | Схема транскрипта + sha256 | дрейф формата = отдельный сигнал | наш `qa_logs.py:290-334` |

---

## 8. Сравнительная таблица

| Система | Подход | Что проверяет | Время | Flaky-защита |
|---|---|---|---|---|
| **Codex** | in-process ядро + `wiremock` SSE; subprocess только для CLI/OTel; snapshot'ы `insta`; nextest-шарды | форма запроса к модели, tool-lifecycle, token-usage, рендер markdown, apply_patch-состояние ФС | unit — сек; suite — мин; полный CI — десятки мин (шарды) | `retries=1`, `slow-timeout 30s terminate-after 2`, test-groups `max-threads=1`, детерминированные pid, нормализатор |
| **Grok Build** | реальный бинарь в PTY + мок-инференс + sandbox `$HOME`; декларативные YAML-сценарии; asciinema | экран/стили/выделение/mouse/resize, инварианты по логу продюсера, p99 кадра | PTY-тест 5–90 с; 176 файлов, `#[ignore]` вне CI | `wait_until_stable`, `Drop`-reap дерева, 4 воркера max, env-стрип двух списков, пер-платформенные baseline |
| **oh-my-pi** | `VirtualTerminal` на ядре kitty; `Bun.Terminal` для реального PTY; in-process сессия; edit-бенчмарк на фикстурах | экран/скроллбафер, edit-качество (формат-эквивалентность), boot-медиана | тест — мс; бенчмарк — мин | мок-провайдер, in-memory settings/auth/session, чанкинг против OOM, hyperfine-медиана локально |
| **Command Code** | публичный headless-контракт (NDJSON + exit-коды); тестов в пакете нет | форму result-фрейма, usage, exit-код | 0.1–10 с | контракт-как-спека; `--max-turns` как детерминированный ограничитель |
| **opencode** | Playwright + мок API на уровне страницы; ACP-subprocess; кассеты; property-тесты | незамоканные маршруты, поток событий, PTY-протокол (property), компоненты | e2e 15 мин/шард; unit 20 мин | 501 на unmocked, кассеты + `CI=true` FAIL, `TestClock`/`ManualClock`, `port:0`, изолированный `XDG_*` |
| **gsd-pi** | реальный бинарь + JSONL-фейк-LLM + изолированный `HOME`; артефакты на тест | форму запроса, tool-цикл, error-пути, состояние проекта + git | 0.2–30 с | структурные ожидания фейка (drift = fail), env-стрип, realpath TMPDIR, `t.after` cleanup |
| **мы (сейчас)** | `qa-tasks/` (12 задач) + `run_suite.py`: 4 лейна (faux/mock/real/daemon), state-oracle, log-invariants, манифест, delta-гейт | состояние ФС, tool-must/must_not, бюджеты запросов/токенов/времени, схема транскрипта, фингерпринт | faux-задача 0.15–4 с; daemon — до 120 с | faux-скрипт, изолированный sandbox + канарейка, `communicate(timeout)`, deadline-поллинг |
| **мы (пробел)** | **нет** одного теста «типовая кодинг-задача → дифф + трейс + стоимость + отсутствие лишних правок» в `cargo test` | — | — | — |

---

## 9. Чего избегать (антипаттерны, подтверждённые источниками)

1. **Точная последовательность tool-вызовов как оракул.** «Fragile, punishes valid unexpected paths» — прямо названо антипаттерном (`qa-tasks/README.md`, «Anti-patterns»; Anthropic-канон в `.research/blackbox-qa-best-practices.md:1.3`). Проверяйте **набор** + инварианты (`must`/`must_not`), а не порядок.
2. **Оракул на текст ответа.** «Never matching the answer text» (`qa-tasks/README.md:6-8`). Текст — только `min_chars`/`max_chars`/`not_contains` как грубые рамки.
3. **`sleep()` как синхронизация.** Заменяется на `wait_until(predicate)`; у opencode прямо: «Never absorb a sleep, poll or production interval with a larger timeout, `--retry`, or `skipIf(win32)`; remove the wait» (`unit-tests SKILL.md:36`).
4. **`maxPasses`/бюджет кадров как часы.** «A bigger budget did not help: it still failed with `maxPasses: 150`, because the keys were lost, not late» (`tui-tests SKILL.md:18`).
5. **`performance.now()`-пороги в обычных тестах.** «Fail on slow Windows runners» (`tui-tests SKILL.md:44`); перф — только в отдельном тире с baseline по платформе.
6. **LLM-judge в блокирующем гейте.** У нас judge — отдельный слой, гейт — детерминированный (`run_suite.py:1748+`, `qa_judge.py`); «Growing this tier into judging» — явно отвергнуто.
7. **Тест, который не может упасть.** «Red-first: a task that cannot fail is dead weight» (`qa-tasks/README.md`, «Adding a task», п.7). У нас это уже реализовано: RT-007 намеренно RED.
8. **Односторонний сьют** (только «должно произойти»). Агент начнёт «всегда/никогда»; нужен баланс `must`/`must_not` (`qa-tasks/README.md`, п.5).
9. **Фиксированный порт.** Windows выдаёт эфемерные с 49152 — можно попасть на занятый (`tui-tests SKILL.md:37`). Всегда `port: 0`.
10. **Общий `$HOME` между прогонами.** «Sharing the host HOME causes ENOTEMPTY races when multiple spawns interleave on a CI runner» (`gsd spawn.ts:50-60`).
11. **Snapshot без нормализации.** Первый же прогон на другой машине убьёт тест (лечение — `normalize.rs`-подход).
12. **Ассерт на сырой ANSI.** «Use `result.stdoutClean`» (`tests/e2e/README.md`, «Anti-patterns»).
13. **Гигантская фикстура «как в проде».** opencode: stress-фикстура с 300-строчными диффами «держала половину всех флейков CI»; mount одного диффа блокировал рендерер на 4–11 с (`app-e2e SKILL.md:60-64`).
14. **Source-grep тесты** вместо поведения (`tests/e2e/README.md`, «No source-grep tests»).
15. **Ассерт на порядок событий стрима CLI.** Эмпирика на нашем бинаре: число `message_update` варьируется 28–31 между прогонами при идентичном результате (§5.7). Порядок — только после фильтра дельт.

---

## 10. ГОТОВЫЙ ПЛАН ДЛЯ НАС

### 10.1 Что именно тестируем (а)

**Задача: `fix_offbyone_add` — «починить функцию и подтвердить тестом».**

Почему такая:
- воспроизводима (файл на 6 строк, никакой сети);
- проверяема детерминированно (содержимое файла + diff-стат инструмента);
- требует **двух разных инструментов** (`python_repl` для чтения/проверки, `edit` для правки) — покрывает и tool-роутинг, и edit-движок;
- имеет честный «красный» контроль (до правки тест падает);
- 0.15–0.3 с на прогон (измерено).

Фикстура (`tests/blackbox/fixtures/fix_offbyone_add/`):

```
work/
  src/calc.py        # def add(a, b): return a - b   ← баг
  tests/test_calc.py # assert add(2,3) == 5
  README.md          # «не трогай меня» — ловушка на scope creep
script.json          # faux-сценарий: 4 турна
expect.json          # ожидаемые инварианты
```

`script.json` (грамматика нашего faux-провайдера, `crates/pa-ai/src/providers/faux/script.rs:175-200`):

```json
{"responses": [
  {"content": [{"type": "toolCall", "name": "python_repl",
                "arguments": {"code": "print(open('src/calc.py').read())"}}]},
  {"content": [{"type": "toolCall", "name": "edit",
                "arguments": {"path": "src/calc.py",
                              "edits": [{"oldText": "return a - b", "newText": "return a + b"}]}}]},
  {"content": [{"type": "toolCall", "name": "python_repl",
                "arguments": {"code": "import subprocess; print(subprocess.run(['python','-m','pytest','-q','tests'],capture_output=True,text=True).stdout)"}}]},
  "Fixed add(): tests pass."
]}
```

> **Важно (проверено):** на print-лейне ростер инструментов — **только `python_repl`** (`crates/pa-cli/src/print_runtime.rs:1408` `builtin_tools` → пустой vec + kernel-tool). `bash` даёт `Tool bash not found` (проверено). Поэтому проверка тестов идёт через `python_repl` + `subprocess`, а не через `bash`.

### 10.2 Как изолировать состояние (б)

Ровно то, что уже делает `base_env` в `scripts/qa/run_suite.py:494-510`, плюс минимум для `cargo test`:

| Переменная | Значение | Зачем |
|---|---|---|
`HOME` | `<tmp>/home` | никакой `~/.prime` |
`PRIME_AGENT_CODING_AGENT_DIR` | `<tmp>/home/.prime/agent` | сессии/логи/БД внутри sandbox (`crates/pa-types/src/platform/dirs.rs:35`) |
`PRIME_AGENT_FAUX_SCRIPT` | `<tmp>/script.json` | детерминированная модель |
`PRIME_AGENT_KERNEL_PYTHON` | `<shared>/kernel-venv/bin/python` | **не** пересоздавать venv на каждый тест (иначе первый прогон качает cpython 29 MiB — измерено: 8 с vs 3.7 с) |
`PI_REQUEST_TIMING` | `1` | `logs/agent.jsonl` с фазами (`request_timing.rs:30`) |
`DO_NOT_TRACK` | `1` | убирает telemetry-баннер в stderr (проверено) |
`NO_COLOR` | `1` | стабильный stdout |
`XDG_DATA_HOME` | `<tmp>/home/.local/share` | |
cwd | `<tmp>/work` | git-репозиторий с одним коммитом (для будущих diff-проверок) |

Стирается: `PRIME_AGENT_*`/`PI_*`/`PA_*`/`RLM_*`, `GH_TOKEN`, `GOOGLE_*`, `ANTHROPIC_*`, `OPENAI_*` (как `SCRUB_KEYS`/`SCRUB_PREFIXES` в `run_suite.py`).

Плюс канарейка: `<tmp>` содержит файл с `QA_CANARY_<rand>`; тест грепает транскрипт и stdout на её отсутствие (переиспользуем приём `run_suite.py:935`).

### 10.3 Какие артефакты собирать (в)

```
<tmp>/artifacts/
  stdout.jsonl          # NDJSON-поток событий (--mode json)
  stderr.txt
  session.jsonl         # <agent-dir>/sessions/<id>.jsonl  (транскрипт)
  agent.log.jsonl       # <agent-dir>/logs/agent.jsonl     (request timing, PI_REQUEST_TIMING=1)
  tool-calls.json       # нормализованная последовательность {name, args_sha, is_error, ms}
  usage.json            # {per_turn:[...], total:{input,output,cacheRead,totalTokens,cost}}
  diffs.json            # из tool_execution_end.details.diffs (наш edit их отдаёт)
  result.txt            # финальный assistant text
  expect.json           # ожидания (см. 10.4)
  verdict.json          # что прошло/что нет — для CI-аннотации
```

Нормализация (обязательна, иначе флейки):
- отбросить `message_update`/`tool_execution_update` из **последовательности** (оставить только как счётчик);
- `session` header: оставить только `version`;
- все абсолютные пути → `<TMP>`;
- `timestamp`, `id`, `parentId`, `pid`, `durationMs` → вырезать;
- `usage.cost` — только если модель в faux имеет `cost` (сейчас 0.0 — сравнивать бессмысленно, но **поле должно присутствовать**).

### 10.4 Assertions (г)

**Уровень 1 — состояние (блокирующий, детерминированный):**

| # | Ассерт | Значение |
|---|---|---|
A1 | `exit_code == 0` | контракт CLI |
A2 | `work/src/calc.py` содержит `return a + b` и **не** содержит `return a - b` | задача решена |
A3 | `work/README.md` побайтово равен исходному | нет scope creep |
A4 | множество файлов в `work/` == исходное (без `__pycache__`, `*.tmp`, `.pytest_cache` — либо чистить в setup, либо разрешать явным списком) | нет мусора |
A5 | `work/tests/test_calc.py` не изменён | агент не «чинил» тест |

**Уровень 2 — трейс (блокирующий):**

| # | Ассерт | Как |
|---|---|---|
B1 | `tool_calls` содержит ровно один `edit` с `path == "src/calc.py"` | по `tool_execution_start` |
B2 | **ни один** `tool_execution_end` не имеет `is_error: true` | `result.is_error` |
B3 | ни одного `edit` по пути вне `src/calc.py` | анти-scope-creep на уровне инструмента |
B4 | `tool_calls` ⊆ `{python_repl, edit}` (ростер не расширился) | ловит случайное включение MCP/субагентов |
B5 | нет `bash` с `rm -rf`, `git checkout --`, `git reset --hard` | `must_not` из нашей же грамматики |
B6 | отфильтрованная последовательность типов событий **стабильна** (см. §5.7) и содержит `tool_execution_start/end` парами | структурный контракт |
B7 | `session.jsonl`: набор `type` == `{session, model_change, thinking_level_change, service_tier_change, custom_message, message}` и роли `message` == `[user, assistant, toolResult, assistant]` | drift-детектор (измерено: 4 message-строки на 2 турна, 10/10 прогонов) |

**Уровень 3 — стоимость/время (advisory, warn-only в PR, блокирующий в nightly):**

| # | Ассерт | Порог (измерено) |
|---|---|---|
C1 | `usage.totalTokens` == 11 466 ± 200 | измерено: 11 466 ровно, 20/20 прогонов |
C2 | `usage.cost.total` присутствует и ≥ 0 | форма контракта |
C3 | wall-clock ≤ 10 с | измерено: 0.15–0.28 с → 40× запас |
C4 | `agent.log.jsonl`: `max_requests ≤ 5` (1 на турн + запас) | бюджет ретраев |
C5 | `agent.log.jsonl`: `max_consecutive_failed_requests == 0` | нет шторма |
C6 | `phase: prompt-built.totalMs ≤ 3000` | фаза сборки промпта не деградировала |

**Уровень 4 — negative controls (обязательны, иначе тест «не может упасть»):**

| # | Контроль | Ожидание |
|---|---|---|
N1 | скрипт **без** edit-турна | A2 падает (файл не изменён) |
N2 | скрипт правит `README.md` | A3 падает + B3 падает |
N3 | скрипт с устаревшим `oldText` | B2 падает (`is_error: true`), A2 падает |

Все три проверены вживую (§5.7) — тест действительно различает.

### 10.5 Как сделать не-flaky (д)

1. **Никакого `sleep`.** Один `Command::output()` (или `communicate(timeout)`) — процесс завершается сам; таймаут 60 с на процесс, 10 с на ассерт-бюджет.
2. **Один прогон = один sandbox.** `tempfile::TempDir` (Rust) / `mkdtemp` — как `base_env`.
3. **Общий kernel venv** через `PRIME_AGENT_KERNEL_PYTHON` (иначе первый прогон 8 с + сеть; проверено).
4. **Фильтр дельт перед сравнением последовательностей.** Это единственный реальный источник вариативности (28–31 событие → 1 последовательность после фильтра; 25/25 прогонов).
5. **Никаких ассертов на `timestamp`/`id`/`pid`/`durationMs`.** Нормализация.
6. **Никаких ассертов на текст ответа.** Только `result.txt` непустой и ≤ 400 символов (форма).
7. **Токены — с допуском ±2%** (faux-модель стабильна, но системный промпт может измениться легитимно; при изменении промпта обновляем константу осознанно).
8. **Последовательный запуск** (`--test-threads=1` для этого файла или `serial_test`), потому что `PRIME_AGENT_*` — process-global; альтернатива — всегда spawn, никогда не `set_var` (как `print_runtime_e2e.rs:16-30`).
9. **Retry НЕ используем.** Если тест флейкует — это баг теста, а не «нестабильность»; Codex-овский `retries=1` — страховка для 10k тестов, а не для одного гейта.

### 10.6 Как встроить в CI (е)

| Тир | Что | Время | Блокирует |
|---|---|---|---|
**smoke** | `cargo test -p pa-cli --test blackbox_coding_task` (1 задача, 1 прогон) | **0.3 с** + сборка | да, каждый PR |
**smoke ×3** | то же, 3 прогона подряд (детект нестабильности) | 0.9 с | да, каждый PR |
**полный** | 5 задач × 3 прогона + negative controls | ~5 с | nightly |
**advisory** | токены/время против baseline (манифест) | +0 | warn, nightly — блокирует при −3pp |

Встраивание:
- новый файл `crates/pa-cli/tests/blackbox_coding_task.rs` — попадёт в существующий шардированный прогон (`.github/workflows/ci.yml:270-300`, 8 шардов по crc32) автоматически, без правок workflow;
- фикстуры — `crates/pa-cli/tests/blackbox/fixtures/<task>/{work/,script.json,expect.json}` (вне `tests/`-компиляции, чтобы не ломать `cargo test --workspace`);
- отчёт — писать `verdict.json` в `CARGO_TARGET_TMPDIR` и печатать markdown-таблицу в stdout при падении (как `xai-grok` `summary_table`);
- **не** добавлять в `qa-tasks/` (это другой тир — поведенческий, с манифестами); новый тест — это **regression keeper** уровня «контракт CLI + edit-движок».

### 10.7 Эмпирическая валидация дизайна (сделано сегодня)

Прогоны на `target/debug/prime-agent` (debug-сборка, не release — цифры консервативны):

| Метрика | Значение |
|---|---|
Edit-only задача (1 турн `edit`), n=20 | медиана **0.146 с**, p95 0.167 с, max 0.212 с |
Задача с 6 правками, n=6 | медиана **0.27 с**, max 0.28 с |
3 турна (`python_repl` → `edit` → `python_repl`) | **3.68 с** (первый прогон с bootstrap venv — 8.0 с) |
`usage.totalTokens` (2 турна), n=20 | **11 466 / 11 485 — 20/20 идентично** |
`usage.totalTokens` (3 турна) | 11 464 / 11 513 / 11 550 / 11 583 |
Схема транскрипта (`session.jsonl`), n=10 | **1 уникальная** последовательность типов строк, 1 набор ключей; роли `message` = `[user, assistant, toolResult, assistant]` |
Сырой NDJSON stdout, n=25 | 28–31 событие (**нестабильно**) |
После фильтра `message_update`/`tool_execution_update`, n=25 | **1 уникальная** последовательность (**стабильно**) |
Negative control: без edit | файл не изменён → A2 падает ✔ |
Negative control: правка `README.md` | `calc.py` не изменён, `README.md` изменён → A3+B3 падают ✔ |
Negative control: устаревший `oldText` | `is_error: true`, «Could not find the exact text … Closest match (93% similar)» → B2 падает ✔ |

Также подтверждено:
- `bash` на print-лейне недоступен (`Tool bash not found`) → `python_repl` обязателен;
- `DO_NOT_TRACK=1` убирает telemetry-баннер из stderr (иначе stderr непуст);
- `PI_REQUEST_TIMING=1` даёт `logs/agent.jsonl` с фазами `prompt-built` / `first-byte` / `first-token` / `stream-done` и полем `usage`;
- `edit` возвращает `details.diffs[{path, diff, firstChangedLine}]` — готовый дифф-оракул без чтения ФС.

### 10.8 Пошаговый план (файлы + время)

| Шаг | Файл | Что | Оценка |
|---|---|---|---|
1 | `crates/pa-cli/tests/blackbox/fixtures/fix_offbyone_add/work/{src/calc.py,tests/test_calc.py,README.md}` | фикстура (6 + 8 + 1 строк) | 20 мин |
2 | `…/fix_offbyone_add/script.json` | faux-сценарий (4 турна) | 15 мин |
3 | `…/fix_offbyone_add/expect.json` | A1–A5, B1–B7, C1–C6 в декларативном виде | 40 мин |
4 | `crates/pa-cli/tests/blackbox/support/mod.rs` | `Sandbox::new()` (env, tempdir, канарейка), `run_task()`, `parse_ndjson()`, `normalize()`, `ToolCallLog`, `UsageSummary` | 2.5 ч |
5 | `crates/pa-cli/tests/blackbox_coding_task.rs` | сам тест: 1 happy-path + 3 negative controls + 3× повтор happy-path | 1.5 ч |
6 | `…/support/verdict.rs` | `verdict.json` + markdown-таблица в stdout при падении | 45 мин |
7 | baseline `expect.json` числа | зафиксировать измеренные 11 466 токенов / 10 с бюджета / ≤5 запросов | 15 мин |
8 | прогон 20× локально (`--test-threads=1`) | подтвердить 0 флейков | 10 мин (2 с прогон) |
9 | (опц.) `scripts/qa/run_suite.py --tasks BB-001` | подключить как faux-задачу в существующий сьют для манифеста/дельты | 1 ч |

**Итого: ~7 часов** (без п.9 — 6 ч). Блокирующий CI-гейт добавляет **+0.3 с** к шарду.

### 10.9 Что даёт больше всего за меньшее (если времени мало)

1. **Шаги 1–5** (≈4.5 ч): один тест, три negative control — это уже 80% ценности.
2. **Negative controls обязательны.** Без них тест «не может упасть» и по канону — мёртвый груз.
3. **Не добавлять PTY.** Для кодинг-задачи PTY не нужен; PTY-тир у нас уже есть (`pa-cli/tests/*_pty_e2e.rs`, `pa-tui/tests/*_headless.rs`).
4. **Не добавлять LLM-judge.** У нас уже есть `qa_judge.py` в отдельном (не-блокирующем) тире.

---

## 11. Ссылки на первоисточники (для дальнейшего чтения)

- Codex: `openai/codex@main`, `codex-rs/core/tests/{common,suite}`, `codex-rs/.config/nextest.toml`, `.github/workflows/rust-ci.yml`, `bazel/rules/e2e_benchmark.bzl`
- Grok: `/tmp/grok-build/crates/codegen/xai-grok-pager-pty-harness/{src,tests,benches}`
- oh-my-pi: `/tmp/omp-src/packages/{coding-agent,tui,ai,typescript-edit-benchmark}`, `scripts/{ci-test-ts.ts,bench-guard.ts}`
- Command Code: `/usr/lib/node_modules/command-code/dist/bundled/command-code-knowledge/reference/headless.md`
- opencode: `sst/opencode@dev`, `.opencode/skills/{unit,tui,app-e2e,component}-tests/SKILL.md`, `packages/{app/e2e,http-recorder,simulation,cli/test}`, `.github/workflows/test.yml`
- gsd-pi: `open-gsd/gsd-pi@main`, `tests/e2e/{README.md,_shared/*}`, `packages/pi-ai/src/providers/fake.ts`, `scripts/audit-test-confidence.mjs`
- Мы: `qa-tasks/README.md`, `scripts/qa/{run_suite,qa_logs,qa_judge}.py`, `.research/blackbox-qa-best-practices.md`, `crates/pa-cli/tests/print_runtime_e2e.rs`

**Внешний канон** (из `.research/blackbox-qa-best-practices.md`): Anthropic «Demystifying evals for AI agents» (pass@k vs pass^k, capability vs regression-сьюты, 20–50 задач из реальных фейлов), Terminal-Bench 2.0 (Docker + pytest-оракул на **состояние** среды), τ-bench (сравнение финального состояния, не текста), CORE (Path Correctness по tool-вызовам как метрика, не как гейт).
