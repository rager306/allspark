# Матрица тестирования форка allspark — полный gap-анализ
## Источники: Codex CLI (Rust, 55KB test harness), oh-my-pi (Bun/TS, mock model + real loop), grok-build (Rust, xai-grok-test-support crate), ruvector ADRs

---

## Что уже есть (не повторять)

| Слой | Тесты | Покрытие |
|---|---|---|
| Unit (6 крейтов) | ~3300 | pa-lsp/graph/edit/mem/ast/diff/agent — каждый модуль |
| Integration (daemon) | ~100 | supervisor, worker lifecycle, protocol, MCP, failover |
| Binary e2e | 1 | fork_tools_e2e: faux model → lsp tool call → real parse |
| Launcher e2e | 3 | substitution, no-seeding, real-binary version |
| Live LSP smoke | 3 | typescript-language-server, gopls |
| Architecture contract | 3 | unsafe scan, dep direction, workspace member |
| Provider failover | 8+3+8 | 429/503/drop-mid-stream/all-fail + wire-level mock |
| Context snapshot | 6 | canonical JSON, normalization |
| MCP conformance | 7 | happy path, SSE flood, invalid method, version mismatch, timeout |
| Multi-client daemon | 5 | exactly-once fan-out, isolation, graceful disconnect |

---

## GAP-АНАЛИЗ: 10 паттернов, которых у нас НЕТ

### P0 — закрывают реальные баги

| # | Паттерн | Из какого проекта | Наш gap | Баги, которые поймает |
|---|---|---|---|---|
| **G1** | **Wire-level mock inference server** — настоящий HTTP/SSE server (не in-process faux), скриптуемый по ходам, с request-log для ассертов | Codex: `tests/common/responses.rs` (55KB) — wiremock + SSE + WS + gated chunks | Наш faux — in-process, не проходит HTTP-стек/SSE-парсер/retry-слой | (1) SSE-парсер рвёт mid-chunk; (2) retry не восстанавливает контекст; (3) 401 не триггерит refresh |
| **G2** | **Context-snapshot** — нормализованный дамп «что ушло в модель» (system prompt + messages + tools) per-turn | Codex: `context_snapshot.rs` (29KB) — rewrite_known_segments, группировка в окна | Мы проверяем наличие tools в промпте, но не байтовый body | (1) Tool definitions поехали после рефакторинга; (2) MCP guidance появился/пропал; (3) system prompt дрейфует |
| **G3** | **Provider failover chaos drill** — 429+Retry-After, 503, connection-drop-mid-SSE, all-fail, circuit-breaker | Codex: `retry_after.rs` (27 тестов), grok: `conversation_replay` + `MockInferenceServer` | У нас есть failover_e2e, но не drop-mid-SSE и не circuit-breaker | (1) Drop-mid-SSE теряет контекст; (2) circuit breaker не открывается; (3) retry storm |
| **G4** | **PTY multi-client leader cluster** — 2+ TUI на 1 daemon, exactly-once fan-out, cross-client events, attach/detach | grok: `leader_pty_e2e/leader_two_clients_shared_session.rs` | У нас multi_client_e2e — но без PTY и без exactly-once assertion | (1) Fan-out дублирует события; (2) detach рвёт сессию; (3) attach не видит историю |
| **G5** | **MCP conformance matrix** — protocol version, SSE flood, invalid method, partial response, reconnect storm | omp: `mcp-reconnect-storm.test.ts` (circuit breaker `RECONNECT_BURST_LIMIT`), `mcp-http-transport` (real Bun.serve) | У нас mcp_product_path_e2e — happy path only | (1) MCP reconnect storm спавнит бесконечные процессы; (2) partial response крешит парсер; (3) version mismatch не обрабатывается |

### P1 — системность

| # | Паттерн | Из какого проекта | Что закрывает |
|---|---|---|---|
| **G6** | **Wire-shape pin** — сериализованный JSON байт-в-байт (включая отказ от `null` в optional полях) | grok: `wire_shape.rs`, `wire_round_trip.rs` | Round-trip без pin пропускает появление/исчезновение `null` |
| **G7** | **RSS/heap soak с числовым гейтом per-cycle** — RSS/threads/fds не растут | grok: `dhat_soak.rs`, `ResourceSnapshot/RssSampler` | Memory leak в daemon/workers при 100+ ходов |
| **G8** | **Type-level conformance** wire ↔ server — `conformance.types.ts` | omp | Дрейф протокола ловится на компиляции |
| **G9** | **Cassette record-replay** — запись реального provider-ответа → CI без ключа | agentverify, toolcontract | Real-model тесты в CI без API ключей |
| **G10** | **`ClientPolicy`/scripted реплики** на elicitation/permission — детерминизм интерактивных веток | grok: `ClientPolicy/ClientHook` | Permission/elicitation ветки не тестируются |

### P2 — полировка

| # | Паттерн | Из | |
|---|---|---|---|
| G11 | `issue-NNNN-repro` именование регрессий | omp | Трассировка |
| G12 | Snapshot-тесты TUI (`insta` crate) | ratatui recipes | User-visible регрессии |
| G13 | `PR_SET_PDEATHSIG` orphan-reap в e2e | grok | CI-гигиена |
| G14 | `dispose`-контракт с проверкой освобождения | omp `mcp-reconnect` | Resource leak |

---

## ПЛАН: P0 тесты по файлам

### Файл 1: `crates/pa-ai/src/testing/mock_inference.rs` (G1)
Wire-level HTTP/SSE mock server:
- `MockInferenceServer::new()` → ephemeral port
- `.queue_json(status, body)` / `.queue_sse(chunks)` / `.queue_drop_after(n)` / `.queue_error(status, code, msg)`
- `.requests() -> Vec<RecordedRequest>` — method/path/headers/body
- SSE: chunked transfer, `data: {...}

` + `[DONE]`
- Drop-mid-SSE: TCP close после N chunks (не clean EOF)
- Тесты: SSE round-trip, drop-mid-SSE, request log, rotation

### Файл 2: `crates/pa-core/src/session_engine/context_snapshot.rs` (G2)
УЖЕ ЕСТЬ (ef1bc9c81). Расширить:
- `.with_tool_definitions()` — tool JSON в снапшот
- `.per_turn()` — снапшот после каждого хода (не только финальный)
- `.compare_golden(golden_file)` — golden-file gate

### Файл 3: `crates/pa-daemon/tests/provider_failover_drill.rs` (G3)
УЖЕ ЕСТЬ (provider_failover_drill_e2e.rs). Расширить:
- Drop-mid-SSE: mock отдаёт 3 chunks → TCP close → assert retry с 3 chunks в контексте
- Circuit breaker: 5 подряд 503 → breaker opens → следующий запрос сразу отклоняется
- Recovery: breaker open → cooldown → half-open → success → breaker closes

### Файл 4: `crates/pa-daemon/tests/multi_client_pty.rs` (G4)
УЖЕ ЕСТЬ (multi_client_e2e.rs). Расширить:
- PTY (не pipe): настоящий терминал + ANSI-рендеринг
- Exactly-once: клиент B видит каждый event ровно один раз (не дубликат, не пропуск)
- Cross-client: A отправляет → B видит update; B отправляет → A видит update
- Detach/attach: A detach → B продолжает; A re-attach → видит пропущенное

### Файл 5: `crates/pa-core/src/mcp/conformance.rs` (G5)
УЖЕ ЕСТЬ (conformance_tests.rs). Расширить:
- SSE flood: 100 notifications подряд → клиент не падает, очередь не переполняется
- Partial response: JSON обрезан mid-key → error, не паника
- Reconnect storm: сервер умирает → клиент reconnect'ится с circuit breaker (≤10 попыток)
- Version negotiation: mismatch → error, не молчаливый downgrade

---

## ПРИОРИТЕТЫ ПО БАГ-ФИНДИНГУ

### Уровень 1 (ловят баги СЕЙЧАС):
1. G1 Drop-mid-SSE в pa-ai stream.rs — stream parser рвётся на chunked
2. G3 Circuit breaker в failover — нет CB, только retry
3. G5 MCP reconnect storm — нет burst limit
4. G2 Context snapshot — tool definitions могли поехать

### Уровень 2 (ловят баги при росте):
5. G4 PTY multi-client — fan-out, isolation
6. G7 RSS soak — memory leak при 100+ ходов
7. G6 Wire-shape pin — protocol drift

### Уровень 3 (полировка):
8. G9 Cassette — CI без ключей
9. G10 ClientPolicy — интерактивные ветки
10. G8 Type conformance — compile-time guard


# Матрица v2 — волна фиксов 2026-10-16 (375ad95cf..74d1e3d04)

## A. Новые регресс-тесты по коммитам (все red-first доказаны)

| Коммит | Тест | Что охраняет |
|---|---|---|
| 2b3d74842 | daemon_rejection_headless::worker_socket_closed_refusal_keeps_the_draft_consumed; prompt_delivery_tests::worker_socket_closed_refusal_is_ambiguous_delivery | упавший воркер ≠ молчаливый ресабмит драфта (дубль хода) |
| 2e79330c3 | in_stream_credit_frame_classifies_as_payment_required; a_signal_present_request_still_times_out_on_a_stalled_peer; every_stream_attempt_carries_a_bounded_request_timeout | платёжный сигнал в SSE → failover, не ретрай-лэддер; таймаут при сигнале; 300s у каждого хода |
| ffd91a66b | a_non_https_installer_url_is_refused_unless_the_hatch_is_open; the_funnel_refuses_a_non_https_url_before_fetching; the_upstream_ts_daemon_socket_is_recognized_by_path_only; the_upstream_ts_daemon_is_never_a_stop_candidate_for_a_fork_root; the_fork_run_skips_the_upstream_ts_socket_and_probes_the_fork_socket | https-only env-URL; форк не убивает демона upstream |
| 58394f09a | kernel_error_clip (3: megabyte/small/fields+boundaries); truncates_raw_payload_on_multibyte_char_boundaries | error-payload клип на settle; UTF-8-границы |
| 734261a93 | loop_rpc_spawned_headless::a_slow_compaction_rebuild_does_not_starve_input | UI-цикл не блокируется RPC компакции |
| 5461ba80a | file_processor (4: oversized/at-cap/giant-line/multibyte) | @file кап 256KiB + маркер |
| 07364009a | compaction estimate-тесты с PNG-заголовками | image-токены ≠ 0 у User |
| 0f7547061 | the_verifier_never_rides_the_authorization_url; the_state_is_an_independent_url_safe_secret | PKCE verifier не в URL |
| abe2a419b | test_repl.py: neutralizes write/append + reopens read_only | restore не обнуляет файлы |
| 37e788625 | golden_bash_group_matches_ts | TZ-независимость golden-хешей |
| 9c6ca829d | pa-jev suite (22); pa-core jev (23) вкл. create_session_installs_jev_tool_only_when_enabled | Judge-слой; гейт default OFF |
| 5122da3d3 | acp/continue/daemon_commands/differential suites | env-изоляция харнессов от ambient PRIME_AGENT_* |

## B. Правила прогона на ЭТОЙ машине (эмпирика)
- Всегда: `PA_TS_BINARY=/nonexistent/ts` (диф-лейны честно скипаются; PATH
  prime-agent = Rust-бинарь, детектор принимает его за TS).
- Дисково: `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
  CARGO_BUILD_JOBS=4` (mold + сотни тест-бинарников с debuginfo рвут диск).
- prime-agent-runtime: `uv run --with pytest python -m pytest` (НЕ `uv run
  pytest`: pytest резолвится вне венва → сабпроцесс без dill).
- Параллельные сборки агентов: чистить target/debug/deps *-[0-9a-f]* >20M и
  /tmp/pa-comp-*, /tmp/pa-worker-compact-* между волнами.

## C. ENV-KNOWN (красные на чистом HEAD, не наши)
- tui_big_streamed_turns_render_at_the_producer_rate — paced-stream vs
  cgroup-CPU квота контейнера (nice −5 не помогает; 45s WaitIdle истекает).
- supervisor_roster::passivation (6), model_resolution (2),
  state_getters ts_shapes, adoption/wake/archive семейство, core_layer
  prompt-doc guard, ssh-verbatim, offline catalog — детерминированно падают
  на 375ad95cf (проверено stash); классы: ambient-env / TS-binary /
  prompt-doc разрыв (#14/#31 волны 2).
- concurrent_update_harness / payload concurrent — флаки параллельности
  (изолированно зелёные).

## D. Разделы pa-jev и rate-limit
- pa-jev Phase 1: см. ADR adr-pa-jev-judge-layer.md; Phase 2 (потребители
  auto-thinking/re-rank/arbiter + SQLite-кэш) — отдельная матрица после
  внедрения, с mini-корпусами по образцу RuFlo ADR-391 (замороженный корпус,
  вслепую, гейт измеренной победы).
- rate-limit admission (wave 2a): см. adr-ratelimit-admission.md; тесты
  M1-M5 прописаны там (AIMD, узкий бракет, очередь+stagger, Retry-After
  дисциплина, budget+breaker).
