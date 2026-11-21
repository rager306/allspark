# Аудит захардкоженных параметров и план выноса моделей в конфиг
Date: 2026-11-21. Tree: `/root/allspark` @ `2879ed31b` (+ незакоммиченные правки путей).

## 1. `crates/pa-ai/src/models_fork.rs` — почему нужен рефакторинг

Состояние: 1 182 строки, 44 модели, 7 провайдеров (`muse` 5, `codebuddy` 15,
`genspark` 16, `agnes` 4, `opencode` 2, `ninerouter` 1, `subquadratic` 1).
Таблица `FORK_RAW_MODELS` — статический Rust-массив; `FORK_MODELS` конвертирует
его в `Model`. Реестр приклеивает таблицу после каталога:
`registry.rs` `.chain(fork_provider_models())`.

Что зашито в таблице: `provider`, `id`, `name`, `api`, `base_url`, `reasoning`,
`thinking_levels`, `input`, `cost`, `context_window`, `max_tokens`, `headers`,
`supports_developer_role`, `max_tokens_field`. Плюс provider-специфичные
константы: `CODEBUDDY_HEADERS` (включая `User-Agent: IDE/2.108.1`), наборы
effort-уровней, `judgment_question_cap("subquadratic") = 16`.

Ключевой факт: формат `models.json` **уже умеет всё нужное** —
`ModelDefinition { id, name, api, baseUrl, reasoning, thinkingLevelMap, input,
cost, contextWindow, maxTokens, headers, compat }`, `ProviderConfig {
baseUrl, apiKey, api, headers, compat, authHeader, models, modelOverrides }`
(`crates/pa-core/src/models/custom.rs:94-150`). `compat` — сырой JSON, поэтому
`maxTokensField`/`supportsDeveloperRole` тоже выразимы. Discovery-слой уже есть:
`doc/discovery.json` + `models/discovery-snapshot.v1.json`
(`crates/pa-models/src/discovery.rs`). Не хватает только загрузки fork-таблицы
из файла.

### Предлагаемая схема

1. Новый ассет `fork-models.v1.json` (схема `providers`, совместимая с
   `models.json`), который содержит текущие 44 записи.
2. Порядок разрешения (первый победил):
   - `$AGENT_DIR/models/fork-models.v1.json` — пользовательский override;
   - `<packageDir>/models.fork.json` — ассет релиза (рядом с `models.bundled.json`);
   - `include_str!` — вкомпилированный fallback для офлайна/первого запуска.
3. `$AGENT_DIR/models.json` остаётся пользовательским слоем с высшим приоритетом
   (уже так: `merge_custom_models` заменяет по `(provider,id)` и добавляет
   остальные).
4. Новые настройки:
   - `[models] forkCatalog: string` — путь/URL к fork-каталогу;
   - `[models] catalogSources: [string]` — дополнительные каталоги (файл/URL),
     по аналогии с `mcpCatalogSources`;
   - `[models] strict: bool` — падать/предупреждать на неизвестных полях.
5. Валидация при загрузке: schema version, дубли `(provider,id)`, известный
   `api`, схема `baseUrl`, обязательные поля. Ошибка должна быть видимой (как
   `CustomModelsResult.error`), а не молчаливым fallback.
6. Кэш: парсить один раз на процесс, ключ — `(path, mtime, size)`.
7. Тесты: parse; precedence (user > package > compiled); битый файл → fallback
   + видимая ошибка; temp-файл добавляет модель без пересборки; каждый
   fork-провайдер имеет хотя бы одну модель.
8. Упаковка: `models.fork.json` в installer layout и release-архив; обновить
   `scripts/package_release.py`, `scripts/release/bundle_catalog.py`,
   `readme-fork.md` и тест layout.

### Найденные дефекты в самой таблице

- `subquadratic/subq-snap-preview`: 64K контекст и $0.05/1M, тогда как актуальная
  `subq-snap-1.2` — 384K и $0.04/1M. Таблица уже отстала (в этом окружении
  исправлено через `models.json`, но новый пользователь получит preview).
- Два имени одного шлюза: `ninerouter` в таблице и `9router` в `models.json` и
  `auth.json`; env-ключ объявлен как `NINEROUTER_API_KEY`, а auth-ключ лежит под
  `9router`. Комментарий обещает `NINEROUTER_URL`, но переменная **нигде не
  читается** — только в комментарии (`models_fork.rs:262`).
- `CodeBuddy` User-Agent версии IDE зашит в код; при смене версии клиента это
  правка бинаря.

Оценка: вынос таблицы — P0, потому что снимает пересборку для любой новой
модели/провайдера и убирает дрейф метаданных.

## 2. Инвентарь захардкоженного (отфильтровано: 1285 `const`, 422 по ключевым словам)

### P1 — влияет на поведение пользователя

| Параметр | Значение | Где | Куда вынести |
|---|---|---|---|
| Provider request timeout | `300_000 ms` | `session_engine/provider_adapter.rs:22` | `[retry.provider] timeoutMs` (ключ уже есть, но **не читается**) |
| Provider retry/backoff classes | quota 30m, rate 30s, concurrent 5s, capacity 45s±15s, server 20s, max pause 7d | `session_engine/provider_retry.rs:286-305`, `provider_park.rs:16` | `[retry.backoff]` |
| Total provider retries | `8` | `session_engine/provider_failover.rs:51` | `[retry] maxTotalRetries` |
| User bash output | 50 KiB / 2000 lines | `pa-daemon/src/user_bash.rs:26-30` | `[limits] bashOutputBytes/bashOutputLines` |
| Kernel/bash output | 64 KiB | `kernel/shared.rs:14,36` | `[limits] kernelOutputChars` |
| Graph output/search | 50 KiB; search limit 200; pack 12/64 | `tools/graph/tool/mod.rs:23`, `tools/graph/args.rs:425-435`, `reports.rs:250-253` | `[graph] outputMaxBytes/searchLimit/packSymbols` |
| Prompt input caps | context file 256 KiB, project 512 KiB, skills 64/64 KiB | `resources/mod.rs:26-45`, `skills/mod.rs:25-31` | `[context] fileBytes/projectBytes/skillsInventory*` (с обязательным отчетом об усечении) |
| Arbiter tuning | 3 заметки, 2.5s, 400 chars, 2 fail | `jev/tool_arbiter.rs:62-76` | `[jev.toolArbiter] maxNotes/timeoutMs/maxCommandChars` |
| Jev timings | rerank 8 кандидатов/3s, cache 8 MiB, cooldown 5m | `jev/graph_rerank.rs:40-45`, `jev/cache.rs:40`, `jev/chain.rs:26` | `[jev] rerankCandidates/rerankTimeoutMs/cacheMaxBytes` |
| Auto-thinking | prompt 2000, timeout 4s | `jev/auto_thinking.rs:32-34` | `[jev.autoThinking]` |
| Autonomous defaults | 3 continuations, 12 turns, 80k tokens, 30m, gate 3/5m | `autonomous/mod.rs:20-34` | `[autonomous]` (сейчас только CLI-флаги) |
| RLM child timeouts | create 120s, prompt/state/kill 30s, watch 60s/2s | `pa-daemon/src/rlm_children.rs:67-89` | `[agents] child*TimeoutMs` |
| Agent messaging | 16 384 chars, 20 pending, 3/s | `session_engine/agent_messaging.rs:16-24` | `[agents.messaging]` |
| Session registry | 100 sessions, 48h TTL | `tool_profile/session_registry.rs:12-14` | `[sessions] maxRegistry/idleTtl` |
| Prime Inference default model | `z-ai/glm-5.3` | `models/resolver.rs:6` | каталог/`[providerDefaults]` |
| Telemetry endpoint | `api.primeintellect.ai/...` | `pa-telemetry/src/sinks/analytics.rs:20` | `[telemetry] endpoint` (self-hosted/offline) |
| Catalog URLs | upstream `prime-agent-catalog` | `pa-models/src/fetch.rs:9-14` | `[models] catalogSources` (сейчас fork не может указать свой каталог) |

### P2 — операционные/нагрузочные

- Kernel: ready/repair/bootstrap 30s, restore 30s, snapshot 5s, shutdown 5s,
  snapshot max 256 MiB/variable 16 MiB — `kernel/shared.rs:15-31`,
  `kernel/state_snapshot.rs:8-10`, `kernel/manager/mod.rs:42-54`.
- Daemon: worker request 30s, create 120s, stop settle 10s, backpressure
  capacities 128/4096/32k, registration backoff 250ms..30s —
  `pa-daemon/src/*` (см. строки выше).
- MCP: probe 15s, OAuth callback base 53700 + 10, local catalog 256 KiB/50 —
  `mcp/probe.rs:21`, `mcp/oauth_callback.rs:20-21`, `mcp/local_catalog.rs:12-16`.
- Traces: 20 MiB trace, 3 retries, 5 req/min, preview 8k —
  `agent_traces.rs:35-47`.
- Update: installer URL `app.primeintellect.ai/...`, download base R2 —
  `update/installer.rs:56-61` (download base уже переопределяется env).
- Compaction: threshold 0.95 уже настраивается (`compaction.thresholdPercent`);
  reserve/keep — тоже.
- LSP: `lsp.servers` уже применяется (`pa-lsp/src/servers.rs`), `idleTimeoutMs`
  читается. Добавлять нечего, кроме документирования.

### P3 — не выносить (инварианты/безопасность)

- `MAX_PROTOCOL_LINE_BYTES`, event queue capacities, lock retry intervals,
  socket peer checks, `MAX_PROJECT_SETTINGS_BYTES`, prompt text/routes,
  `config` security gates. Это внутренние границы, их изменение не «гибкость»,
  а риск.

### «Настройка есть, но не работает»

- `retry.provider.timeoutMs` — объявлен в `Settings` и игнорируется; реальный
  таймаут зашит в `provider_adapter.rs:22`.
- `sessionArchiveMaxAgeDays` / `sessionArchiveMaxSessions` — мёртвые ключи,
  потребителей нет (удалены из рабочего конфига 2026-11-21).
- `NINEROUTER_URL` — обещан комментарием, не читается.
- `codebuddy`/`genspark` метаданные (цены, контекст) — статическая копия
  провайдерского ответа; дрейфует без пересборки.

## 3. Предлагаемая схема настроек (набросок)

```json
{
  "models": {
    "forkCatalog": "~/.allspark-home/.prime/agent/models/fork-models.v1.json",
    "catalogSources": ["https://.../catalog.v1.json"]
  },
  "limits": { "bashOutputBytes": 51200, "bashOutputLines": 2000, "kernelOutputChars": 65536 },
  "context": { "fileBytes": 262144, "projectBytes": 524288, "skillsInventory": 64 },
  "retry": { "provider": { "timeoutMs": 300000, "maxRetryDelayMs": 60000 } },
  "jev": { "toolArbiter": { "maxNotesPerRule": 3, "timeoutMs": 2500 } },
  "graph": { "searchLimit": 200, "outputMaxBytes": 51200 },
  "agents": { "childCreateTimeoutMs": 120000, "messageMaxChars": 16384 },
  "telemetry": { "endpoint": "https://api.primeintellect.ai/api/v1/agent-analytics/events" }
}
```

## 4. Порядок внедрения

1. **P0**: вынести `models_fork` в `fork-models.v1.json` + runtime override
   (тест «новая модель без пересборки»).
2. **P1a**: подключить `retry.provider.timeoutMs` и вынести backoff-классы.
3. **P1b**: `[limits]`/`[context]` (с отчётом об усечении, как требует контракт).
4. **P1c**: `[jev.toolArbiter]`, `[graph]`, `[agents]`.
5. **P2**: сгруппировать таймауты kernel/daemon под `[timeouts]`, но только те,
   что реально нужны оператору.
6. Каждый шаг — red-first тест: значение из настроек меняет наблюдаемое
   поведение, а не только парсится.

## 5. Риски

- Вынос prompt-кэпов: AGENTS требует «cap и report»; настройка не должна
  позволять неограниченное чтение. Верхний предел оставить в коде.
- Вынос security/invariant-констант запрещён: настройка не должна ослаблять
  peer-проверки, лимиты протокола, границы доверия.
- `models.json` — уже пользовательский контракт; новый fork-каталог должен
  использовать ту же схему, иначе появятся два формата.
- Кэш каталога: после правки файла нужна инвалидация по mtime, иначе «добавил
  модель, а её нет» повторит класс `graph.auto = "watch"`.

## 6. Implementation status (2026-11-21)

Landed in this tree (cold `cargo check`, targeted tests, clippy `-D warnings`, `dist` build all green):

- **P0 model catalog**: `$AGENT_DIR/models/fork-models.v1.json` (or beside `models.json`) is
  read on every registry load; precedence is compiled fork table -> runtime fork catalog ->
  `models.json`. Adding a fork model no longer needs a rebuild. Tests:
  `fork_catalog_adds_a_model_without_rebuilding`,
  `models_json_still_wins_over_the_fork_catalog`.
- **Compiled SubQ entry**: `subq-snap-1.2`, 384K context, $0.04/1M input, text+image.
- **Settings reload without a daemon restart**: TUI `/reload` now reloads MCP, refuses while a
  turn is busy, and retires the built runtime so the next demand seam rebuilds the tool roster
  and prompt from disk; the Python namespace is snapshotted (`KernelDisposition::Snapshot`).
  Tests: `reload_refuses_while_a_turn_is_busy` and the existing reload/faux tests.
- Verified: the `dist` binary carries `update_plan` and the `graph` `trace` route; the route
  test passes.

Still open (P1/P2, additive, no build blocker):

- Wire `retry.provider.timeoutMs` (currently dead; 300_000 ms is compiled in
  `provider_adapter.rs`), and expose the backoff classes.
- `[limits]`: user-bash 50 KiB/2000 lines, kernel 64 KiB, graph 50 KiB; `[context]` caps.
- `[jev.toolArbiter]` (3 notes, 2.5 s, 400 chars) and `[graph]` search/pack/output limits.
- `[agents]` child timeouts/messaging caps; `[telemetry] endpoint`; model-catalog URLs.
- Automatic (turn-boundary) settings check; today `/reload` is explicit.
