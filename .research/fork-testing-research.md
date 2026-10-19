# Тестирование coding-agent инструментов: Codex CLI / oh-my-pi / grok-build
## Gap-анализ для форка allspark (`/root/prime-agent`)

> **Дата**: 2026 · **Ветка/ревизия**: main (read-only snapshot) · **Метод**: GitHub API (tree + raw), EXA search, локальный `/tmp/omp-src`,
> `crates/ruvector/**/docs/adr/` · **Режим**: read-only research
>
> **Скоуп**: как **Codex CLI** (openai/codex, Rust, 129k★), **oh-my-pi** (can1357, Bun/TS, локально),
> **grok-build** (xai-org, Rust, 27k★) тестируют свои инструменты (TUI, provider, MCP, session),
> и что из этого **не хватает** нашему форку.

---

## 0. TL;DR — 10 паттернов, которых у нас нет

| # | Паттерн | Источник | Ценность для allspark | Приоритет |
|---|---|---|---|---|
| 1 | **Mock-inference server + scripted conversation replay** (мок отвечает на `/v1/responses` и `/v1/chat/completions`, скриптуется по ходам) | Codex `responses.rs` (55 KB), grok `conversation_replay.rs` | Заменяет faux на **wire-level** мок: реальный HTTP/SSE/WS путь | **P0** |
| 2 | **Context-snapshot / request-history snapshot** (что именно ушло в модель — нормализованный дамп по ходам) | Codex `context_snapshot.rs` (29 KB) | Ловит регрессии промпта/tools **на проводе**, не в юните | **P0** |
| 3 | **MCP conformance suite** (общий Python-сервер + матрица профилей protocolVersion, MRTR, SSE-flood, cursor) | Codex `scripts/mcp_conformance/*` (50 KB) | У нас MCP-тесты product-path, нет **конформанса** | **P0** |
| 4 | **Provider-failover «drill»** по фиксированному списку fault-shapes (429+Retry-After, 503, slow headers, drop-after-N-chunks, all-fail) | Codex `retry_after.rs` (27 тестов), grok/INFRO | У нас `provider_failure_e2e`/`failover_e2e` — близко, но без drop-mid-stream | **P0** |
| 5 | **Heap/RSS soak с числовым гейтом** (dhat + RSS/threads/fds per-cycle) | grok `dhat_soak.rs`, `test_subagent_soak.rs`, `resources.rs` | У нас `tui_rss_stability` — только TUI, без subagent-чейна | **P1** |
| 6 | **PTY-harness + leader cluster (2–N клиентов на 1 демоне, exactly-once fan-out)** | grok `leader_pty_e2e/*` | Прямо наш gap «2 TUI на 1 демона» | **P0** |
| 7 | **Wire-shape pin tests** (сериализованный JSON пиннится байт-в-байт, включая отказ от `null`) | grok `wire_shape.rs`, `wire_round_trip.rs` | У нас `daemon_wire_roundtrip.rs` — только round-trip, без pin | **P1** |
| 8 | **Type-level conformance между сгенерированным wire-типом и серверным** | omp `rpc-wire/conformance.types.ts` | Дёшево, ловит дрейф протокола на компиляции | **P1** |
| 9 | **Cassette / record-replay с реальным провайдером** (запись один раз → CI без ключа) | agentverify, toolcontract, demi | Единственный способ закрыть gap «real model + real tool» | **P1** |
| 10 | **Chaos-инъекция на HTTP-слое** (не трогая исходники) + fault taxonomy (crash/omission/value) | AgentChaos (arXiv 2608.06790), agent-chaos | Систематизация provider-failover за пределами 429/503 | **P2** |

---

## 1. Codex CLI (openai/codex) — Rust

### 1.0 Масштаб (из GitHub tree, main)
- **9 251** blob-файлов в `codex-rs/`; **~1451** inline `*_tests.rs`, **644** файла в `tests/`-каталогах,
  **1 588** `.snap`-снапшотов `insta`.
- `codex-rs/core/tests/suite/` — **210** файлов; `app-server/tests/suite/` — **146**;
  `exec/tests/suite/` — **18**; `tui/tests/suite/` — **12**.
- Раннер: `cargo nextest` (быстрый, изолированный, параллельный); `insta` для снапшотов;
  `just test`.
- **Схемы протокола**: `app-server-protocol/schema/` = **318 JSON + 748 TS** сгенерированных артефактов
  (публичный контракт для клиентов).

### (а) Как тестируют

**1. Единый in-process harness `core_test_support::test_codex` (`TestCodexBuilder`).**
Огромный (57 KB) builder: `with_model`, `with_config`, `with_thread_store`, `with_auth`,
`with_models_manager`, `with_workspace_setup`, `with_home`, `with_exec_server_url`,
`with_user_instructions_provider`, `with_external_time_provider`, … и терминалы сборки:
- `build(&server)` — против `wiremock::MockServer`;
- `build_with_streaming_server(&StreamingSseServer)` — пофреймово-управляемый SSE;
- `build_with_websocket_server(&WebSocketTestServer)`;
- `resume(server, home, rollout_path)` и `restart(server, previous)` — **перезапуск сессии с
  тем же `home`/rollout**, ключ к тестам resume/fork/compaction-на-рестарте.

**2. Мок модели — на уровне HTTP/SSE/WS (`tests/common/responses.rs`, 55 KB).**
Не faux-провайдер, а `wiremock` + настоящие SSE/WebSocket-серверы. Хелперы-конструкторы событий:
`ev_response_created`, `ev_function_call(call_id, name, args)`, `ev_function_call_with_namespace`,
`ev_custom_tool_call`, `ev_apply_patch_custom_tool_call`, `ev_exec_command_call`,
`ev_reasoning_item`, `ev_web_search_call_done`, `ev_image_generation_call`, `ev_completed_with_tokens`.
Маунт: `mount_sse_once`, `mount_sse_sequence`, `mount_response_sequence`, `mount_function_call_agent_response`.
Инспекция: `ResponseMock::single_request()`, `requests()`, `saw_function_call(call_id)`,
`function_call_output_text(call_id)`, `assert_parent_turn`, `assert_root_turn`.
`StreamingSseServer` даёт **gated per-chunk** доставку (`StreamingSseChunk { gate, body }`) —
можно тестировать поведение агента *во время* стрима (частичный ответ, отмена, параллелизм).

**3. Контекст-снапшоты (`tests/common/context_snapshot.rs`, 29 KB).**
`SnapshotEntry::captured/body/items/labeled` + `ContextSnapshotOptions`:
`rewrite_known_segments()` (свернуть известные блоки в теги `<PERMISSIONS_INSTRUCTIONS>`),
`include_request_settings()`. Группировка запросов в окна, нормализация путей/ID, рендер
«что ушло в модель» и `insta::assert_snapshot!`. Пример (`scenarios.rs`):
`code_mode_mcp_schema_limits_preserve_explicit_overrides` — снапшот истории запросов.
Правило репо: **любое изменение user-visible UI → обязательный `insta`-снапшот**.

**4. Сценарии «shipped feature» (`suite/scenarios.rs`, 109 KB, 24 теста).**
`astra_kickoff_with_skills_plugins_and_remote_compaction`, `astra_continues_after_a_stream_is_interrupted`,
`astra_refreshes_plugin_tools_and_skills_in_an_existing_thread`,
`model_catalog_refresh_preserves_tools_and_history`, `prediction_fork_inherits_parent_context` —
многоходовые end-to-end прогоны через mock-server со снапшотом request-history.

**5. Provider failover / retry (`suite/retry_after.rs`, 27 тестов).**
Три уровня: чистый unit (`#[tokio::test(start_paused = true)]` — виртуальное время!) через
`run_with_retry(RetryPolicy{...})` с проверкой задержек в бэндах `FIRST_RETRY_MIN/MAX_DELAY`;
и интеграционные с `RetryTelemetryCapture` (tracing-Layer, слушающий `retry.attempt`,
`retry.delay_ms`, `retry.layer`, `retry.operation`). Покрыто: 429 `Retry-After`, 5xx,
transport-failure без расхода бюджета, WS `nested retry-after`, terminal-классификация
(`flex_unavailable`), `Retry-After` побеждает локальный backoff, компакция v2.
Отдельно `provider_failover` моделируется двумя mock-провайдерами (у нас это уже есть).

**6. MCP — многослойно.**
- Юнит-крейт `codex-rs/codex-mcp/src/*_tests.rs` (auth, binding, catalog, elicitation, pagination,
  connection_manager, sandbox_metadata, trusted_access, **streamable_http_retry**).
- Интеграция `codex-rs/rmcp-client/tests/` (mcp_2026_stdio, mcp_2026_sse_discovery, mrtr,
  **stdio_message_limits**, stdio_stderr_cleanup, process_group_cleanup,
  streamable_http_recovery, oauth_*).
- **Тестовые серверы-бинарники**: `rmcp-client/src/bin/{rmcp_test_server, test_stdio_server,
  test_streamable_http_server, test_mcp_2026_stdio_server, test_mcp_2026_discovery_stdio_server}.rs`
  (`test_stdio_server.rs` — 42 KB).
- **MCP conformance suite**: `scripts/mcp_conformance/{server.py (50 KB), test_codex_compliance.py (40 KB),
  test_official_conformance.py, test_review_regressions.py}` + `sdk/python/tests/test_mcp_conformance_fixtures.py`,
  `sdk/typescript/tests/mcpConformance.test.ts`. Профили: `DEFAULT_PROFILE`, `REVIEW_PROFILE`,
  `CATALOG_MAX_PROFILE`, `CATALOG_OVER_LIMIT_PROFILE`, `SSE_COMMENT_FLOOD_PROFILE`,
  `REPEATED_CURSOR_PROFILE`, `MISMATCHED_DISCOVERY_ID_PROFILE`. Тесты: modern discovery stateless,
  MRTR требует capability, subscription ack+close, legacy probe fallback, **SSE comment flood**,
  integer precision (no float rounding!), two-page cursors, untrusted origin.
- `mcp_tool_cache.rs` (4 теста): `mcp_calls_stay_bound_to_each_thread`, ленивый старт для субагентов,
  eager для root.

**7. Параллелизм инструментов (`suite/tool_parallelism.rs`).**
Барьер-синхронизация внутри тестового инструмента (`barrier { id, participants, timeout_ms }`)
+ измерение wall-clock (`assert_parallel_duration`, `< 1600 ms`) → доказывает реальное перекрытие,
а не просто «нет ошибки».

**8. Регистрация tools как телеметрия (`tool_registration_metrics.rs`).**
`InMemoryMetricExporter` + `MetricsClient::with_runtime_reader()`; проверяют, что **stream-retry
не раздувает** число регистраций и что hosted-tools не попадают в inventory.

**9. Реальные модели — только opt-in.**
`core/tests/suite/live_cli.rs`: `#[ignore]`, требует `OPENAI_API_KEY`, спавнит **реальный бинарь**
через `codex_utils_cargo_bin::cargo_bin`, стримит stdout/stderr вживую. `skip_if_no_network!`
макросы (`skip_if_target_windows!`, `skip_if_remote!`, `skip_if_wine_exec!`) — герметичность в CI.

**10. App-server (protocol-level) тесты.**
`app-server/tests/common/test_app_server.rs` (84 KB) — клиент к **реальному процессу** по stdio
и WebSocket; `rpc_delay.rs` — **delay-interposer** (прокси, добавляющий задержку в WS-поток)
для тестов гонок; `mock_model_server.rs`, `models_cache.rs`, `auth_fixtures.rs`.
191 файл app-server тестов, включая `v2/thread_fork.rs` (105 KB), `v2/thread_resume.rs`,
`v2/mcp_protocol_default.rs`, `connection_handling_websocket.rs`.

**11. Окружения.** `test_environment.rs`: `TestTargetOs {Linux, MacOs, Windows}` ×
`TestEnvironment {Local, Docker{container}, WineExec}` — один и тот же suite гоняется в
локальной/Docker(Linux)/Wine(Windows) среде.

### (б) Что перенять (конкретно)

1. **Заменить faux-only e2e на wire-level mock.** У нас faux — in-process, он не проходит
   HTTP-стек, SSE-парсер, retry-слой, сериализацию тела. Codex доказывает то же самое, но через
   `wiremock`/`StreamingSseServer` — тестируется и транспорт.
2. **Контекст-снапшоты.** Самый дешёвый способ ловить «промпт/tools поехали» — у нас есть
   `prompt_guards.rs`, но он про *наличие* tools, не про *байтовый* request-body.
3. **`start_paused = true` для retry-тестов.** Виртуальное время убирает 13-минутный флак.
4. **Тестовые серверы-бинарники для MCP** + conformance-профили (у нас есть `fixture-echo`, но
   нет протокольных профилей и flood/limit-кейсов).
5. **`assert_parent_turn`/`root_turn_id`** — каноническая метадата хода; полезно для
   мультиагентных цепочек (наши rlm_children).
6. **Snapshots как gate для TUI.** У нас headless-тесты TUI есть, но без `insta`-снапшотов
   user-visible вывода.

### (в) Конкретные тесты для allspark (Codex-производные)

| Тест | Файл | Что делает | P |
|---|---|---|---|
| `provider_wire_failover_drop_mid_stream` | `crates/pa-daemon/tests/provider_failover_e2e.rs` | мок отдаёт 2 чанка, затем рвёт соединение → assert retry + `auto_retry_end` + один outcome-row | P0 |
| `retry_ladder_uses_virtual_time` | `crates/pa-daemon/src/...` unit | `#[tokio::test(start_paused=true)]`, проверка задержек в бэндах | P0 |
| `request_body_snapshot_after_prompt_change` | `crates/pa-core/tests/` | снапшот request-body (tools+system) на 3 хода | P0 |
| `mcp_conformance_modern_and_legacy_profiles` | `crates/pa-core/tests/mcp_conformance.rs` | профили protocolVersion + MRTR + cursor | P0 |
| `mcp_sse_comment_flood_is_bounded` | там же | flood SSE-комментариями не вешает клиент | P1 |
| `mcp_integer_precision_preserved` | там же | большие int в schema/результате без float-rounding | P1 |
| `tool_registrations_stable_across_stream_retry` | `crates/pa-ai/tests/` | счётчик регистраций не растёт при ретрае | P1 |
| `two_tools_overlap_proven_by_barrier` | `crates/pa-agent/tests/agent_loop_scripted.rs` | барьер + wall-clock < N ms | P1 |

---

## 2. oh-my-pi (can1357/oh-my-pi) — Bun/TypeScript

### 2.0 Масштаб (локально `/tmp/omp-src`)
- **3 024** `*.test.ts`. Ключевые: `packages/agent/test/` (52), `packages/coding-agent/test/` (**~700**),
  `packages/mnemopi/test/` (~75), `packages/tui/test/` (~270), `packages/ai/test/`.
- Раннер: `bun:test` (`describe/it/expect`), `bun test --parallel`.

### (а) Как тестируют

**1. `createMockModel` — mock-провайдер как первоклассный гражданин (`packages/ai/src/providers/mock.ts`, 17 KB).**
Три режима: массив `responses`, async-generator (можно `await` между ходами!), per-call
`handler(context)`. Отдаёт `{ content, stopReason, usage, stopDetails, serviceTier, responseId,
fallbackCreditHandle }`. Инспекция: `mock.calls`. Ключевой паттерн (см. `sent-tool-definitions.test.ts`,
`prompt-tools-loop.test.ts`): **mock model + real tool + real `agentLoop`** — SUT это настоящий
цикл агента, мокается только сеть.

**2. Tool-protection / pruning (`tool-protection.test.ts`).**
`pruneToolOutputs(entries, tokenizer, config)` и `collectShakeRegions(...)`: проверяют, что
**`skill://`-результаты не прунятся/не «трясутся»**, а обычные `read` — да. Т.е. защита
контекста тестируется как контракт на селектор.

**3. In-band tool dialects (`prompt-tools-loop.test.ts`).**
Для «owned dialects» (glm/qwen/hermes) — реальный `agentLoop` исполняет `<tool_call>` из текста,
**снимает нативные tools с провода**, перекодирует историю в текст, но внутренний store
остаётся каноническим (`toolCall` блок + `toolResult`). Это тест «двойного представления».

**4. MCP — mock-transport + реальные транспорты.**
`mcp-test-utils.ts`: `createMockTransport(responses: Map<method, unknown[]>, onRequest)` — счётчик
вызовов, очередь ответов на метод, ошибка «No mock response for X call #N».
Покрытие: `mcp-reconnect.test.ts` (retriable errors, dedup имён, deferred tools),
`mcp-http-transport.test.ts` (real `Bun.serve`, initialized-before-GET-SSE, stalled body),
`mcp-stdio-transport.test.ts` (Windows PATHEXT/cmd-shim resolution!), `mcp-json-rpc.test.ts`
(redactUrlForLog — маскирование секретов в query).
**`mcp-reconnect-storm.test.ts` — образцовый chaos-тест**: фикстура, которая делает
initialize+tools/list и сразу выходит; ассерт `spawns <= 10` (circuit breaker `RECONNECT_BURST_LIMIT`
+ 1 connect + 1), ждёт `getConnectionStatus() === "disconnected"` **вместо blind-sleep**, документирует
регрессию issue #1592 (66 487 процессов за 7 минут).

**5. Session lifecycle — concurrency и утечки.**
`agent-session-concurrent.test.ts` (90 KB, **35 тестов**): session_stop feedback, TTSR
continuations, abort-гонки, `collapseSchedulerSettleDelays()` (спай `scheduler.wait` → 1 хоп).
`agent-session-dispose-releases-memory.test.ts`: после `dispose()` пусты `messages`,
`sessionManager.getEntries()`, `rawSseDebugBuffer`, `appendOnlyContext` — прямая регрессия issue #8003
(29 GB RSS на 20-часовой сессии).
`session-manager-close-race.test.ts`: детерминированно открывает окно гонки через parked-deferred
на `NdjsonFileWriter.close()`.
`agent-session-payload-rejection-413.test.ts` (71 KB): 413 → компакция → ретрай; «Compaction freed
too little context to make progress».

**6. RPC wire — два независимых уровня.**
- `rpc-wire/conformance.types.ts` (14 KB) — **type-only конформанс** между сгенерированным
  `rpc-wire.generated` и серверными типами: outbound (каждый ключ на проводе, leaf-значения
  укладываются), inbound, дискриминаторы open-union. Проверяется `bun check`.
- `rpc-wire/generated.test.ts` — «committed bundle == fresh `bun run gen:rpc`».
- `rpc-wire/fake-openai-server.ts` — **скриптованный OpenAI-совместимый сервер для smoke-тестов
  Rust/Go-клиентов**: `bun test/rpc-wire/fake-openai-server.ts <agentDir>` слушает loopback,
  пишет `models.yml`, печатает `READY <port>`. Гоняет **реальный `omp --mode rpc`** полные ходы
  с host-tool calls.
- `rpc-client.restart.test.ts` — реальный клиент против `fixtures/mock-rpc-agent.ts`
  (v2 negotiation, oversized 270k-emoji response, legacy state normalization).

**7. TUI — `VirtualTerminal` на настоящем терминальном ядре.**
`packages/tui/test/virtual-terminal.ts` (17 KB): WASM-сборка kitty `screen.c`+`vt-parser.c`
(`kitty-vt-wasm`). Grapheme-aware: VS16/ZWJ/wide-char. `interactive-terminal-e2e.test.ts`:
реальные `AgentSession`+`InteractiveMode`+`Composer` против `VirtualTerminal(120,32)`,
`term.resize`, `waitForRender(predicate)`, ассерты «ровно один MARKER_DRAFT в viewport и scrollback».
`render-stress-scheduler.ts`, `virtual-render-scheduler.ts` — стресс-рендер.

**8. Provider parity.**
`mnemopi/provider-all-15-tools-parity.test.ts` — 23 инструмента, проверка **наличия каждого имени +
JSON-round-trip схемы**; `*-parity` файлов 12 (theme-highlight, task-executor-mcp, catalog-compat,
native-vector, beam, identity-memory, cli-errors, cli-stats…). Идея: parity-тест как
«список ожидаемых имён + схема», дёшево ловит пропажу tool.

**9. Реальные провайдеры — OAuth из окружения.**
`packages/ai/test/tokens.test.ts`: резолвит OAuth (`anthropic`, `github-copilot`, `google-gemini-cli`,
`google-antigravity`, `openai-codex`), стримит реально, абортит на 1000 символов и проверяет
**семантику usage при abort по провайдерам** (openai отдаёт usage только в финальном чанке →
при abort `usage.input == 0`; anthropic/google — раньше → `> 0`). Это ровно «cache/usage correctness
против реального провайдера».

**10. Issue-репро как отдельный жанр.** ~90 файлов `issue-*-repro.test.ts` — каждая
закрытая регрессия = именованный тест с номером issue. Плюс `repro-issue-*.test.ts`.

### (б) Что перенять

1. **`createMockModel` с async-generator и per-call handler** — мощнее нашего
   `ALLSPARK_FAUX_SCRIPT` (массив). Позволяет await-синхронизацию между ходами.
2. **Type-level wire conformance** (`conformance.types.ts`) — наш `daemon_wire_roundtrip.rs`
   runtime-only; TS-версия ловит дрейф на компиляции.
3. **`fake-openai-server.ts` + real-binary RPC smoke** — наш `fork_tools_e2e` использует faux;
   omp поднимает **реальный HTTP-сервер** и гоняет **реальный бинарь**.
4. **VirtualTerminal на kitty-VT** — наш TUI тестируется headless, но не через настоящее
   VT-ядро (grapheme/wide-char/BCE-семантика).
5. **Reconnect-storm с circuit breaker** — прямой аналог для наших MCP/daemon.
6. **`dispose`-контракт с проверкой освобождения памяти** — у нас `session_stop_lifecycle_e2e`,
   но не «после dispose все структуры пусты».
7. **issue-NNNN-repro как именование** — дисциплина трассировки регрессий.

### (в) Конкретные тесты для allspark (omp-производные)

| Тест | Файл | Что делает | P |
|---|---|---|---|
| `rpc_wire_type_conformance` | `crates/pa-types/tests/` | type-level: серверные типы ⊆ wire-типы | P1 |
| `daemon_wire_shape_pinned` | `crates/pa-types/tests/daemon_wire_roundtrip.rs` | пин JSON-формы (не только round-trip) | P1 |
| `fake_provider_server_drives_real_binary` | `crates/pa-cli/tests/fork_tools_e2e.rs` | заменить faux на loopback OpenAI-сервер | P0 |
| `session_dispose_releases_all_state` | `crates/pa-daemon/tests/session_stop_lifecycle_e2e.rs` | после dispose пусты messages/entries/buffers | P1 |
| `mcp_reconnect_storm_is_capped` | `crates/pa-core/tests/` | краш-фикстура + ассерт spawn-лимита | P1 |
| `tui_exactly_once_under_resize` | `crates/pa-tui/tests/` | один MARKER в viewport+scrollback | P2 |
| `usage_semantics_on_abort_per_provider` | `crates/pa-ai/tests/` | abort → cache/usage по провайдеру | P2 |

---

## 3. grok-build (xai-org/grok-build) — Rust

### 3.0 Масштаб (GitHub tree, main)
- **4 487** blob; **1 148** тест-файлов. 79-пакетный workspace.
- `crates/codegen/xai-grok-pager-pty-harness/tests/` — **40** `pty_e2e/` + **7** `leader_pty_e2e/`
  + 13 корневых. `xai-grok-shell/tests/` — ~40. `xai-grok-test-support/src/*_tests.rs` — 23.
- Есть крейты `xai-grok-test-support` (публичная тест-библиотека) и `xai-test-utils`.

### (а) Как тестируют

**1. `xai-grok-test-support` — целый тест-фреймворк как крейт.**
- `GrokStdioClient` — драйвит `grok agent stdio` как subprocess через `acp::ClientSideConnection`,
  авто-аппрувит permission, собирает streamed-чанки; `ClientPolicy`/`ClientHook` (scripted
  реплики на elicitation/permission/trust).
- `AcpTestClient` — **verbatim JSON-RPC строки** для форм, которые typed-клиент не умеет
  (Foundation `\/`-методы, string-UUID id).
- `MockInferenceServer` — мок `/v1/chat/completions`, `/v1/responses`, `/v1/messages` с
  **request log** и per-conversation скриптами; отдаёт и Chat Completions, и Responses SSE.
- `Conversation`/`ConversationReplay`/`ConversationScript` — **скриптовый реплей по ходам**:
  `MockToolCall`, `Tool::Read/Shell` (мок сам подставляет имя инструмента, которое предлагает
  запрос), `pinned_turn` (закрепить ход за запросом), failure-ответы in-place.
  27 тестов реплея (`conversation_replay_tests.rs`): parallel calls, reasoning usage/cost,
  resend idempotency, unpinned handover, registration panics.
- `TestSandbox` (изолированные пути, hermetic child env, git), `TestProcess` (detached lifecycle,
  teardown process-tree, bounded output tails), `run_headless`, `git_workdir`, `grok_binary`.
- `ResourceSnapshot`/`ResourceGrowth`/`RssSampler` — **RSS/threads/fds** через `xai_tty_utils`
  (тот же сэмплер, что в проде), фоновый поток-сэмплер с peak.
- `MockOtelServer`/`OtelRecorder` — OTLP/HTTP коллектор, читает экспортированные логи/метрики/трейсы.
- `MockManagedConfigServer`/`TestSigningKey` — политика managed-конфигурации.
- `spawn_counting_server` — HTTP/1.1 сервер, считающий соединения (для wire/pooling).
- `env_parse`, `tracing_capture`.

**2. PTY-harness + leader cluster — ключ к «N клиентов на 1 демоне».**
`leader_pty_e2e/leader_two_clients_shared_session.rs`: A спавнит leader, отправляет ход,
B **attach**-ится и должен увидеть транскрипт A **ровно один раз** (не дублировать replay),
ход 2 из A стримится в B, ход 3 из B — в A (driver/viewer flip), затем `drop(a)` — B должен
выжить. `leader_n_clients_shared_session.rs`: 1 driver + N viewers, **exactly-once** для
replay и live fan-out, settle-then-count («pump может только показать дубль, не спрятать»).
`common.rs`: `DEFAULT_ROWS/COLS`, `turn_sentinel(n)` (короткий, чтобы не переносился),
`WELCOME_SCREEN_SENTINEL`, таймауты 240s/120s.

**3. `test_leader_stdio_integration.rs` (125 KB, 56 тестов).**
Прямой in-process сервер leader + `LeaderClient`: `test_two_clients_session_isolation`
(ID-неймспейсинг `client_id|original_json`, каждая нотификация только своему клиенту),
`test_multiple_clients_same_message_ids`, `test_server_exits_when_all_clients_disconnect`,
`test_session_ownership_cleanup_on_disconnect`, `test_cancel_severed_in_swap_window_reaches_agent_after_recovery`,
`test_hung_agent_leaves_transport_healthy_and_forwards_cancel`, `test_sever_mid_rpc_orphans_response_and_replay_recovers`.

**4. Soak-тесты с числовым гейтом.**
- `dhat_soak.rs` (`xai-grok-shell`): `dhat::Profiler::builder().testing()`, warmup=16, measure=256,
  гейты `MAX_BLOCKS_PER_SESSION = 0.5`, `MAX_BYTES_PER_SESSION = 1024.0`; печатает
  `DHAT_SOAK_SUMMARY {json}` **перед** ассертом; `quiesce()` (50 yield + 50ms sleep + 50 yield).
- `test_subagent_soak.rs` (32 KB): `SUBAGENT_SOAK_CYCLES=20000`, метрики RSS/threads/fds,
  `Bounds::from_env()`, warmup = `MAX_COMPLETED_ENTRIES`, `MetricValue` trait с деструктуризацией
  (новое поле = ошибка компиляции, а не молчаливый пропуск метрики), budgets per-platform.
- `tui_rss_stability` (у нас уже есть) — аналог.

**5. Wire-shape pin (`wire_shape.rs`, `wire_round_trip.rs`).**
`tool_config_entry_serializes_to_pinned_json_shape` — точный `json!({...})` байт-в-байт;
`minimal_entry_deserializes_from_id_only` (sparse payload); `explicit_null_map_is_rejected`
(пиннит отказ от `null`); `workspace_request_round_trips_for_every_variant` — **для каждого
варианта enum** round-trip с rich payload; пиннинг serde-тегов (`adjacent tag`).

**6. Реальный бинарь + мок inference.**
`test_built_binary_e2e.rs`: smoke (`--version`), ACP stdio полный lifecycle; ловит
dynamic-linking failures (libgit2/OpenSSL), session-init краши, protocol-регрессии; `#[ignore]`
по умолчанию, `GROK_BINARY` в CI.

**7. Resource hygiene / orphan-reap.**
`orphan_reap.rs`: `PR_SET_PDEATHSIG(SIGKILL)` на PTY-child — если тест-раннер убит (CI timeout),
**ядро** реапит ребёнка; проверяет `pid_is_running` + `/proc/<pid>/stat` starttime (не спутать с
переиспользованием PID).

**8. Прочее.** `repro_sse_flood.rs` (throttled-клиент не ломает здоровый сервер, ровно 1 GET),
`counting_server.rs`, `perf_harness` (timestamps каждого client-callback в `Recorded` для
latency-анализов), `mcp_toggle_gateway_persist.rs`, `session_create_subphase_timers.rs`,
`tool_call_trace.rs`, `synthetic_*.json` фикстуры.

### (б) Что перенять

1. **Leader-cluster PTY-тест** — прямой ответ на наш gap «2 TUI на 1 демона + exactly-once».
2. **Числовые soak-гейты per-cycle** — «RSS/threads/fds/blocks не растут» как ассерт.
3. **Wire-shape pin** (не только round-trip) — включая отказ от `null`.
4. **`MockInferenceServer` с request-log** — наш мок отвечает, но не даёт log для ассертов.
5. **`PR_SET_PDEATHSIG` orphan-reap** — гигиена CI (у нас e2e спавнят бинари).
6. **`ClientPolicy`/scripted реплики** на elicitation/permission/trust — детерминизм
   интерактивных веток без TUI.
7. **`MetricValue` через деструктуризацию** — новое поле метрики не теряется молча.

### (в) Конкретные тесты для allspark (grok-производные)

| Тест | Файл | Что делает | P |
|---|---|---|---|
| `two_clients_one_daemon_exactly_once` | `crates/pa-cli/tests/` (pty) | 2 TUI на 1 демона: replay ровно 1×, live fan-out, пережить exit драйвера | P0 |
| `n_clients_fanout_exactly_once` | там же | 1 driver + N viewers, settle-then-count | P1 |
| `daemon_client_disconnect_cleanup` | `crates/pa-daemon/tests/` | владение сессией снимается при disconnect | P1 |
| `subagent_churn_heap_steady_state` | `crates/pa-daemon/tests/` | N циклов spawn/complete/evict, гейт blocks/bytes per cycle | P1 |
| `daemon_wire_shape_pinned_and_null_rejected` | `crates/pa-types/tests/` | пин JSON + отказ `null` | P1 |
| `orphan_pty_child_reaped_on_runner_kill` | `crates/pa-cli/tests/` | PDEATHSIG-контракт | P2 |
| `mock_inference_request_log_asserts` | `crates/pa-ai/tests/` | log запросов для ассертов формы | P1 |

---

## 4. ADR / testing-strategy паттерны

### 4.1 Из внешних ADR (EXA)
- **`ruvnet/agent-harness-generator` ADR-010 «TDD Test Contracts»** — три концентрических кольца:
  *unit* (London-school, mock-first) → *integration/contract* (реальный граница, мок внешних) →
  *e2e/real-host smoke* (nightly + pre-release). Contract-фикстура живёт **один раз в ядре**,
  каждый адаптер её импортирует (Fowler «Contract Test»). Каждый ADR заканчивается
  **Test Contract** — списком тестов, без которых решение не считается shipped.
- **`leynos/axinite` ADR-008 «Mutation testing with cargo-mutants»** — nightly 03:00 UTC,
  `cargo mutants --test-tool nextest --features test-helpers`, `continue-on-error: true`
  (не гейтит PR), артефакт `mutants-report` 14 дней, скоуп = файлы, изменённые за 24 ч
  (`git log --since`), ручной `workflow_dispatch(branch, paths)`. Мотивация: coverage ≠ fault-detection.
- **`getwyrd.dev` ADR-0009 «Deterministic Simulation Testing»** — FoundationDB-style: весь
  узел в single-threaded simulated world (madsim/turmoil), любой баг воспроизводится из seed;
  компоненты пишутся против абстрактных time/network/disk trait'ов **с дня 1** (retrofit почти
  невозможен). Ключевая мысль: DST — это **commitment архитектуры**, не тест.
- **Signal Fish Server «Formal Verification»** — шесть слоёв: TLC model-checking (state machine),
  trace-refinement, proptest-инварианты, **proptest wire round-trips** (JSON+MessagePack),
  **proptest fuzz hardening** (парсер не паникует на hostile bytes), e2e/golden. Явно обосновано,
  почему proptest, а не cargo-fuzz/SMT/Kani/Loom.

### 4.2 Из ruvector ADR (локально, `/root/vendor-source/ruvector`)
- `crates/ruvector-mincut/docs/adr/ADR-001-anytime-valid-coherence-gate.md` +
  `DDC-001-coherence-gate-design-criteria.md` + `ADR-002-addendum-sota-optimizations.md` —
  **критерии приёмки как дизайн-артефакт** (`DDC-*` = Design Decision Criteria) с версионной
  историей.
- `crates/rvf/docs/adr/ADR-031-rvcow-branching-and-real-cognitive-containers.md`.
- В корне `docs/adr/` — 107+ ADR (ADR-284 «RVF Execution Contract», ADR-308 «WorldCycle-Style
  Verification for the Physical Action Loop», ADR-252 «agent-memory-compaction»,
  ADR-033 «progressive-indexing-hardening»). Паттерн: **каждый ADR = Status/Context/Decision/
  Verification**, а «verification» — исполняемая часть.

### (б) Что перенять
1. **Test Contract внутри ADR** — наш формат ADR должен требовать раздел «Tests» (какие тесты
   доказывают решение).
2. **Contract-фикстура в ядре, импортируется адаптерами** — применимо к нашим provider-адаптерам
   (`pa-ai/providers/*`) и tool-наборам.
3. **Mutation testing nightly, non-gating** — `cargo-mutants` на `pa-core`/`pa-edit` даст
   «какие ассерты слабые», без гейта PR.
4. **Property-based wire round-trips + fuzz-hardening** — расширить наши `proptest` (сейчас
   только geometry + supervisor) на wire/JSONL-парсеры и `pa-edit` hashline.
5. **DST-commitment** — если когда-нибудь захотим детерминизм daemon-гонок, писать против
   абстрактных trait'ов сейчас (иначе retrofit невозможен).

### (в) Конкретные тесты
| Тест | Файл | P |
|---|---|---|
| `wire_roundtrip_proptest` (все enum-варианты) | `crates/pa-types/tests/` | P1 |
| `jsonl_parser_never_panics` (proptest hostile bytes) | `crates/pa-types/tests/` | P1 |
| `hashline_parse_proptest` | `crates/pa-edit/tests/` | P2 |
| mutation-nightly workflow | `.github/workflows/` | P2 |

---

## 5. Agent-testing best practices (внешние фреймворки)

### 5.1 `agentverify` (pytest-плагин)
Тестируется **траектория действий**, не «вайб». `ExecutionResult` (tool_calls, steps, usage, cost,
latency). Ассерты: `assert_tool_calls` (exact order / subsequence / set, regex/wildcard-аргументы),
`assert_no_tool_call` (safety: инструменты, которые нельзя звать никогда), `assert_cost`,
`assert_final_output`. Ключевое: **cassettes** (человекочитаемый YAML) — записал раз, реплей в CI
без ключа; «step N's input references data produced by step M» — проверка потока данных между шагами.

### 5.2 `toolcontract` (Pact для tool-calls)
Триггер: сменилась версия модели → изменилось ли tool-calling поведение? Golden-набор
expected tool-call траекторий → live-модель → type-aware компараторы → **PASS / FAIL+diff /
INCONCLUSIVE**. **Verification ledger**: контракт перепроверяется против живой модели только
когда изменился контракт/схема/провайдер/строка модели — не на каждом CI.

### 5.3 `agent-evals`
Три правила: (1) **evidence и assertion — раздельные артефакты** (fixture = что произошло,
content-addressed; policy = что разрешено; фикстуры никогда не содержат вердиктов);
(2) **effect replay — integrity gate** (каждая фикстура ре-исполняется против свежего in-memory
tool-server и обязана воспроизвести результаты/финальное состояние — подделка `granted: true`
умирает здесь); (3) **регрессия требует статистики** (paired bootstrap CI + Benjamini-Hochberg).
Вердикты: REGRESSION / IMPROVEMENT / PASS / INCONCLUSIVE.

### 5.4 Chaos / failover drills
- **INFRO «Failover drills»**: каждый drill — эксперимент (steady-state → hypothesis → one fault →
  compare from request-records). Таблица fault-shapes: **429+Retry-After** (следующий target),
  **503/500**, **slow headers** (per-attempt timeout), **headers then silence** (idle limit),
  **drop after N chunks** (error event, no `[DONE]`), **every target failing** (clear error).
  Покрывать faults *до первого события*, *во время стрима* и *по всей цепочке*; проверять по
  request-записям, а не по отсутствию ошибок.
- **`reaatech/agent-chaos`**: 8 типов инъекций (latency, timeouts, rate limits, malformed output,
  token exhaustion, stale context, contradictory results, partial failures), middleware-
  перехватчик, YAML-сценарии, glob-таргеты, cascading failures, JUnit/HTML отчёты.
- **AgentChaos (arXiv 2608.06790)**: fault-taxonomy **crash / omission / value** на content и
  tool-call полях; инъекция на **HTTP-слое без правки исходников**; 65 конфигураций; pass@1
  падает до −50 п.п.; диагностика fault-type < 53%, fault-step < 56%.

### (б) Что перенять
1. **Cassette-слой** — единственный практичный способ «real model + real tool» в CI без ключа.
2. **Ledger-гейт** — перепроверять контракт только при изменении (иначе дорого/флак).
3. **Evidence≠verdict** — фикстуры траекторий отдельно от policy.
4. **Fault-taxonomy** (crash/omission/value) — систематизировать наши provider-fault кейсы.
5. **Statistical gate** для sampled-тестов (у нас их пока нет — но пригодится для eval).

### (в) Конкретные тесты
| Тест | Файл | P |
|---|---|---|
| `provider_cassette_replay` (запись 1× → CI offline) | `crates/pa-ai/tests/` | P1 |
| `tool_call_trajectory_policy` (no-tool-call / порядок / аргументы) | `crates/pa-agent/tests/` | P1 |
| `provider_fault_matrix_crash_omission_value` | `crates/pa-ai/tests/` | P1 |

---

## 6. Gap-анализ: наш форк vs три инструмента

Наша база (по `TEST-MATRIX.md` + инвентарь): **213** test-файлов, **~1090** test-fn в `tests/`,
плюс inline-юниты; **1** binary-level e2e (`fork_tools_e2e` — faux+real lsp+real binary);
**1** arch-contract (`pa-lsp`); live-smoke LSP (real TS server); launcher-e2e; proptest (2 места);
`differential_cli` vs TS-бинарь; `export_live_differential`; goldens; `provider_failure_e2e` +
`provider_failover_e2e`; `mcp_product_path_e2e` (real kernel + stdio fixture); `tui_rss_stability`.

| Gap (из задания) | Есть ли у нас | Есть ли у них | Что взять |
|---|---|---|---|
| **Real model + real tool (не faux)** | ❌ (только faux/scripted; `tokens.test.ts`-аналога нет) | omp `tokens.test.ts` (real OAuth+abort); Codex `live_cli.rs` (ignored); grok `test_built_binary_e2e` (real bin + mock) | cassette-слой + opt-in `#[ignore]` real-key тест |
| **Provider failover chaos (circuit breaker, retry storm)** | ⚠️ частично (`provider_failure_e2e`, `provider_failover_e2e`, storm_429) | omp `mcp-reconnect-storm` (breaker); Codex `retry_after.rs` (27, virtual-time); INFRO fault-shapes | drop-mid-stream; virtual-time ladder; breaker-лимит |
| **MCP roundtrip через реальный сервер** | ✅ `mcp_product_path_e2e` (real kernel+stdio fixture) | Codex conformance suite (профили/flood/MRTR); grok `repro_sse_flood`; omp mock-transport+HTTP/stdio | conformance-профили, flood/limit, integer-precision |
| **Long-session компакшн + memory leak** | ⚠️ `compaction_mega_trace_e2e` (measure, ignored); `tui_rss_stability` | grok `dhat_soak`+`test_subagent_soak` (числовые гейты); omp `dispose-releases-memory` | per-cycle гейт на subagent/session чейне |
| **Concurrent session isolation (2 TUI на 1 демона)** | ⚠️ второй attach в `compaction_abort_e2e`/`completion_stall` | grok `leader_pty_e2e` (2–N клиентов, exactly-once); `test_two_clients_session_isolation` | полноценный 2-TUI PTY-тест |
| **Wire protocol fuzzing** | ❌ (есть `kernel_protocol_frame_caps` — oversized line, не fuzz) | Signal Fish proptest fuzz-hardening; Codex schema-gen (318 json/748 ts) | proptest на JSONL/wire-парсеры |
| **Cache statistics correctness vs real usage** | ⚠️ faux симулирует prompt-cache; `cached_tokens` в моках | omp `tokens.test.ts` (real, per-provider abort-семантика); letta usage-tracking (per-provider mapping) | cassette + per-provider usage-mapping ассерт |
| **Context/request-body snapshots** | ⚠️ `prompt_guards.rs` (наличие tools) | Codex `context_snapshot.rs` + `insta` | байтовый снапшот request-body |
| **Type-level wire conformance** | ❌ | omp `conformance.types.ts` | type-level конформанс |
| **Wire-shape pin** | ⚠️ round-trip only | grok `wire_shape.rs` (pin + null-reject) | pin-тесты |
| **Mutation testing** | ❌ | axinite ADR-008 (cargo-mutants nightly) | nightly non-gating workflow |
| **Orphan/process hygiene** | ⚠️ Drop-kill в тестах | grok `orphan_reap` (PDEATHSIG) | PDEATHSIG на pty-child |
| **Property-based (расширение)** | ⚠️ 2 места | Signal Fish (6 слоёв), Codex/omp | wire/hashline/diff |

---

## 7. Приоритизированный план тестов для форка

### P0 — закрыть главные дыры (сначала)
1. **`provider_wire_failover_drop_mid_stream`** — мок отдаёт 2 чанка, рвёт соединение →
   assert retry + один outcome-row. Расширение `provider_failover_e2e.rs`.
2. **`retry_ladder_virtual_time`** — `#[tokio::test(start_paused=true)]` + бэнды задержек
   (перенос Codex `http_retry_backoff_exhausts_attempts`).
3. **`request_body_snapshot`** — нормализованный дамп request-body на N ходов (перенос
   `context_snapshot.rs`); расширение `prompt_guards.rs`.
4. **`mcp_conformance_profiles`** — Python-сервер с профилями (modern/legacy/MRTR/cursor/flood)
   + матрица; перенос `scripts/mcp_conformance`.
5. **`fake_provider_server_drives_real_binary`** — loopback OpenAI-совместимый сервер +
   реальный бинарь (перенос omp `fake-openai-server.ts`); замена faux в `fork_tools_e2e`.
6. **`two_clients_one_daemon_exactly_once`** — PTY-тест 2 TUI на 1 демона (перенос grok
   `leader_pty_e2e`).

### P1 — системность
7. `subagent_churn_heap_steady_state` (per-cycle гейт blocks/bytes/RSS/threads/fds).
8. `session_dispose_releases_all_state` (после dispose пусты messages/entries/buffers).
9. `daemon_wire_shape_pinned_and_null_rejected` + `rpc_wire_type_conformance`.
10. `provider_cassette_replay` (запись 1× → CI offline) + `tool_call_trajectory_policy`.
11. `provider_fault_matrix_crash_omission_value` (fault-taxonomy).
12. `mcp_sse_comment_flood_is_bounded` + `mcp_integer_precision_preserved`.
13. `tool_registrations_stable_across_stream_retry`.
14. `two_tools_overlap_proven_by_barrier`.
15. `wire_roundtrip_proptest` + `jsonl_parser_never_panics`.

### P2 — полировка/гигиена
16. `orphan_pty_child_reaped_on_runner_kill` (PDEATHSIG).
17. mutation-testing nightly workflow (`cargo-mutants`, non-gating).
18. `usage_semantics_on_abort_per_provider`.
19. `tui_exactly_once_under_resize` (VirtualTerminal/kitty-VT-семантика).
20. ADR-раздел «Test Contract» как обязательный (формат).

---

## 8. Инфраструктурные переносы (что построить)

| Элемент | Из | Куда в allspark |
|---|---|---|
| Mock-inference server с request-log + scripted conversation | grok `mock_server.rs` (45 KB), Codex `responses.rs` | `crates/pa-ai/tests/common/mock_inference.rs` |
| Context/request-body snapshot + нормализация | Codex `context_snapshot.rs` | `crates/pa-core/tests/common/context_snapshot.rs` |
| PTY-harness + leader cluster | grok `pager-pty-harness` | `crates/pa-cli/tests/pty_harness/` |
| Resource-soak helper (RSS/threads/fds + dhat) | grok `resources.rs` | `crates/pa-daemon/tests/common/resources.rs` |
| Cassette-слой (record/replay) | agentverify/demi | `crates/pa-ai/tests/cassettes/` |
| Type-level wire conformance | omp `conformance.types.ts` | `crates/pa-types/tests/wire_conformance.rs` |
| MCP conformance profiles | Codex `scripts/mcp_conformance` | `crates/pa-core/tests/mcp_conformance/` |
| Fault-injection middleware | agent-chaos / AgentChaos | `crates/pa-ai/tests/common/chaos.rs` |
| Snapshot-gate для TUI (`insta`) | Codex | `crates/pa-tui/tests/` |

---

## 9. Источники

**Codex CLI** — `openai/codex` @ main (GitHub API tree: 9251 blob): `codex-rs/core/tests/common/{responses.rs,
test_codex.rs, streaming_sse.rs, context_snapshot.rs, lib.rs, test_environment.rs}`;
`codex-rs/core/tests/suite/{retry_after, tool_harness, tool_parallelism, tools, mcp_tool_cache, compact,
resume, fork_thread, token_budget, prompt_caching, models_cache_ttl, scenarios, tool_registration_metrics,
live_cli}.rs`; `codex-rs/app-server/tests/common/{test_app_server.rs, rpc_delay.rs}`;
`codex-rs/rmcp-client/src/bin/*`; `scripts/mcp_conformance/*`;
`codex-rs/app-server-protocol/schema/{json,typescript}/` (318+748).
Доки: openai-codex.mintlify.app/contributing/testing, deepwiki.com/openai/codex/9.2-testing-infrastructure,
openai.com/index/unlocking-the-codex-harness/.

**oh-my-pi** — локально `/tmp/omp-src` (3024 `*.test.ts`): `packages/ai/src/providers/mock.ts`;
`packages/agent/test/{prompt-tools-loop, sent-tool-definitions, tool-protection, agent-loop}.test.ts`;
`packages/coding-agent/test/{mcp-reconnect-storm, mcp-reconnect, mcp-http-transport, mcp-stdio-transport,
mcp-json-rpc, mcp-test-utils, agent-session-concurrent, agent-session-dispose-releases-memory,
session-manager-close-race, agent-session-payload-rejection-413, interactive-terminal-e2e,
rpc-wire/{conformance.types,generated.test,fake-openai-server}, rpc-client.restart}.test.ts`;
`packages/tui/test/virtual-terminal.ts`; `packages/mnemopi/test/provider-all-15-tools-parity.test.ts`;
`packages/ai/test/tokens.test.ts`.

**grok-build** — `xai-org/grok-build` @ main (GitHub API tree: 4487 blob, 1148 тест-файлов):
`crates/codegen/xai-grok-test-support/src/{lib,resources,mock_server,conversation,conversation_replay_tests,
acp_wire_tests,acp_scripted_client_tests}.rs`; `crates/codegen/xai-grok-pager-pty-harness/tests/{leader_pty_e2e,
pty_e2e,orphan_reap,pty_auto_mode}.rs`; `crates/codegen/xai-grok-shell/tests/{test_built_binary_e2e,
test_leader_stdio_integration,perf_harness}.rs`; `crates/codegen/xai-grok-shell/src/agent/mvp_agent/tests/dhat_soak.rs`;
`crates/codegen/xai-grok-tools/tests/test_subagent_soak.rs`; `crates/codegen/xai-grok-tools-api/tests/wire_shape.rs`;
`crates/codegen/xai-grok-workspace-types/tests/wire_round_trip.rs`; `crates/codegen/xai-grok-mcp/tests/repro_sse_flood.rs`;
`crates/common/xai-test-utils/src/*`. Доки: deepwiki.com/xai-org/grok-build/9.2-session-and-integration-tests,
thegrokbook.com/journey/16-working-on-the-codebase, igniting.github.io/coding-cli/.

**ADR / best practices** — `ruvnet/agent-harness-generator` docs/adrs/ADR-010-tdd-test-contracts.md;
`leynos/axinite` docs/adr-008-mutation-testing-with-cargo-mutants.md; getwyrd.dev/adr/0009;
ambiguous-interactive.github.io/signal-fish-server/architecture/formal-verification/;
локально `/root/vendor-source/ruvector/{docs/adr, crates/ruvector-mincut/docs/adr, crates/rvf/docs/adr}/`;
`simukappu/agentverify`; `Divyansh2202/toolcontract`; `qasimmahmood95/agent-evals`;
infro.io/blog/llm-failover-drills; `reaatech/agent-chaos`; arXiv 2608.06790 (AgentChaos);
`letta-ai/letta` tests/integration_test_usage_tracking.py; `wspl/demi` scripts/compaction-fixture/verify-compaction.ts;
`can1357/oh-my-pi` issue #8003; `openclaw/openclaw` PR #72413.
