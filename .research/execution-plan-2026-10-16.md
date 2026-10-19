# Консолидированный план: review-фиксы + pa-jev (2026-10-16, v2 после верификации)

HEAD 375ad95cf, ветка feat/isolated-defaults. Дерево чистое.

## Источники
- Ревью CE (52 находки) + верификация: verify-p0.md, verify-reliability.md,
  verify-unbounded.md, verify-security.md (+ ожидаются verify-contract-ux.md,
  verify-upstream-wire.md, verify-ts-era.md).
- Jev: adr-pa-jev-judge-layer.md + ruvector-jev-openjev.md + ruflo-jev-scan.md.
- Существующая матрица: fork-testing-matrix.md (14 гэпов, прошлый цикл).

## Итоги верификации (кратко)
- Из 3 P0 ревью настоящая критическая — только #3 (exactly-once). #1 = DOC-CONFLICT
  (код верен контракту AGENTS.md, док врёт), #2 = PARTIAL (решение о модели изоляции).
- Подтверждены к исполнению сейчас: #3, #7, #8, #11, #19, #20, #21, #22, #9 (тест),
  #27+#1 (док-синхронизация под модель A).
- Отложены до вердиктов: #4/#13/#6 (parity-решение от verify-ts-era), #14-17, #30-33,
  #23, #38, #26, #36, #40, #42, #45.
- #12 — REFUTED (не чинить).

## Волна 1 (параллельно, непересекающиеся области; БЕЗ коммитов — коммитит родитель)

| ID | Область | Находки | Суть фикса | Регресс-тест |
|----|---------|---------|-----------|--------------|
| fix-exactly-once | pa-daemon routing + pa-tui prompt | #3 | WORKER_SOCKET_CLOSED классифицировать как ambiguous (is_daemon_rejection не гасит prompt_may_have_been_sent); не восстанавливать драфт поверх частично исполненного хода | red-first: упавший воркер ≠ ресабмит промпта |
| fix-provider | pa-ai + pa-core adapter | #7, #8 | (a) is_payment_signal-пин в anthropic_sse_error; (b) timeout(select_cancel()) в http.rs по образцу h2.rs + timeout_ms в provider_adapter | payment-фрейм в SSE → PaymentRequired; сигнал+таймаут → таймаут срабатывает |
| fix-tui | pa-tui | #20, #21 | rebuild_transcript/refresh_stats → spawn + epoch guard + канал (образец spawn_transcript_backfill); enhanced_keys → spawn как hyperlinks; кэш settings | wedged-воркер не морозит UI-цикл; ни один telemetry await в run-loop |
| fix-update | pa-cli + pa-daemon paths | #19, #22 | https-only для env-URL апдейта (+checksum), fork-сокет allspark-<uid> в стоп-кандидатах, гейт update-лейнов на fork-identity | http-URL отклоняется; стоп-проба находит fork-сокет, не убивает upstream |
| fix-kernel | pa-core kernel | #11, #9 | клип ename/evalue/traceback на settle (образец Result-клипа); multibyte-тест truncation | traceback >cap клипится; тест на UTF-8 границе |
| fix-docs | docs/ | #27, #1 | fork-identity.md переписать под фактическую модель A (AGENTS.md+лаунчер); убрать обещания ALLSPARK_*/~/.allspark; задокументировать реальную изоляцию | — |
| jev-phase1 | НОВОЕ: pa-jev + pa-core::jev | ADR | крейт pa-jev (wire-типы 3 примитивов, JevClient, chunking≤4, retry, Usage), Judge/ChainJudge (native-only, кулдаун 5мин, answer-cache), настройки [jev] default OFF, JevTool, wiring | wire encode/decode фикстуры; кулдаун; native-only; кэш; гейт auto |

Общие правила для всех: #![forbid(unsafe_code)], fmt+clippy -D warnings по своим крейтам,
тесты без sleep (готовность по наблюдаемому), форк-код маркируется (readme-fork.md
дополняет jev-phase1), коммитов НЕТ — родитель ревьюит diff и коммитит по одному
логическому изменению.

## Волна 2 (после вердиктов contract-ux / upstream-wire / ts-era)
- #23 O_TRUNC + #38 armed-route: если verify-ts-era найдёт фикс TS-транка — портировать
  рецепт; иначе писать самим + кандидат в upstream issue.
- #14 lsp/graph contract, #16 MCP-login, #17 dirty-tree, #30-33 prompt/MCP/RLM,
  #26 AGENTS.md chain, #36/#40/#42/#45 wire — по вердиктам.
- #4/#13/#6 — по parity-решению (порт TS-бага → upstream issue + локальный фикс
  только при явном решении пользователя).

## Волна 3 (фан-ин)
- make check (fmt+clippy+tests workspace) целиком.
- Последовательные коммиты: один фикс = один коммит, ambient identity, без трейлеров.
- Обновление матрицы тестирования: fork-testing-matrix.md → расширить находками
  ревью (новые красные тесты) + раздел pa-jev (wire/chunking/retry/cooldown/cache/gate).
- Обновление readme-fork.md (новые настройки [jev], JevTool, изменения update/стоп-проб).

## Порядок коммитов (родитель)
1. fix-exactly-once (P1, единственный «настоящий» P0-класс)
2. fix-provider (#7+#8)
3. fix-update (#19+#22 — безопасность)
4. fix-kernel (#11+#9)
5. fix-tui (#20+#21)
6. jev-phase1 (новая фича, отдельный коммит + readme-fork)
7. fix-docs (док-синхронизация)


## Обновление волны 2 (после verify-upstream-wire, 2026-10-16)
Подтверждены и queued: #23 (добавлен в волну 1 — fix-snapshot-otrunc), #36 (alias
«web-socket»→«websocket» + канонизация + round-trip тест + persist WebsocketCached),
#42 (KNOWN_COMMAND_TYPES += factory_activity, worker_idle_passivation), #45 (CLI
клампит как TUI/TS), #40 (worker обслуживает reattach: same-id → attach с
cancelled:false; cross-id → как plane) — по одному малому PR.
Координация с upstream (НЕ чинить в одностороннем порядке): #38 armed-route (#3312
открыт — наш фикс = parity-divergence), #26 AGENTS.md chain (byte-for-byte TS v0.9.8
дизайн — односторонний кап ломает паритет промпта). Оба — кандидаты в upstream
issue/PR от форка.


## Обновление после verify-ts-era (2026-10-16, финальное)
- Issue-трекер upstream: «closed» ≠ fixed — 14/15 закрыты NOT_PLANNED vouch-ботом.
  Все parity-вердикты ниже проверены кодом TS v0.9.8 (/tmp/prime-ts-ref) + форка.
- #23 (O_TRUNC): рецепт ГОТОВ в теле issue #3447 (skip io.IOBase при save;
  restore: w/a/+ → /dev/null) — передан fix-snapshot-otrunc. После фикса:
  переоткрыть upstream issue с нашим патчем.
- #38 (armed-route #3312): рецепт = Solution из issue + несмерженный PR #24
  (eligibility по модели провалившегося message, budget zero-delay re-routes,
  4 RED-теста). Код — upstream-наследие (не регрессия форка) → wave 2,
  координация с upstream. Бонус: providerBackupModel парсится, читателя нет.
- #4, #13, #11, #6 — ВСЕ «TS-баг-перенесён-верно» (кандидаты upstream issue):
  #4 @file (TS читает целиком+verbatim), #13 image-токены (user=0/flat 4800 —
  вплоть до комментария), #11 traceback (kernel-кап только 1MiB/строку),
  #6 PKCE state=verifier (в TS и upstream main идентично). Решение по локальным
  фиксам #4/#13/#6 — за пользователем (parity-divergence); #11 — локальный
  hardening допустим (клип на settle не меняет wire для нормальных размеров),
  fix-kernel работает.
- Форк-специфичных регрессий среди проверенного НЕТ; repl.py байт-идентичен upstream.


## РЕШЕНИЕ пользователя (2026-10-16): parity-тройку чиним локально
#4 (@file кап), #13 (image-токены), #6 (PKCE state) — локальные фиксы
(parity-divergence принят осознанно) + upstream-репорты потом.
Очередь спавна (по одному при освобождении слота, rate-limit дисциплина):
- СЕЙЧАС: fix-pkce #6 (codebuddy) — малый, isolated
- СЛЕДУЮЩИЕ: fix-filecap #4 (pa-core file_processor), fix-imgtokens #13
  (pa-core compaction) — спавнить по мере завершения текущих.

## Волна 3 (фан-ин, после всех фиксов) — ОБНОВЛЕНО:
1. make check (fmt+clippy+tests) целиком.
2. Полная ПЕРЕПРОВЕРКА всех фиксов (re-verify: каждый red-тест зелёный,
   diff соответствует вердиктам) — отдельный субагент-валидатор.
3. Upstream-репорты (копим патчи, готовим к отправке):
   - #3447 kernel O_TRUNC: наш патч + рецепт из issue (переоткрыть).
   - #3312 armed-route failover: Solution + PR #24 рецепт.
   - #4 @file unbounded, #13 image tokens, #11 traceback caps, #6 PKCE
     state=verifier — «faithful TS port» баги, репорт с патчами.
   - #38 провайдер: providerBackupModel парсится без читателя.
4. Матрица тестирования: fork-testing-matrix.md расширить (новые красные
   тесты всех фиксов + раздел pa-jev + раздел rate-limit admission).
5. readme-fork.md: [jev] настройки, JevTool, update/стоп-пробы, fork-identity.


## Фан-ин: правила диска (эмпирика 4 переполнений)
- cargo test --workspace на этой машине требует CARGO_PROFILE_DEV_DEBUG=0
  CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=4 (mold параллельно линкует сотни
  тест-бинарников 30-100MB; с debuginfo не влезает в свободные ~25G).
- Регулярная чистка: target/debug/deps *-[0-9a-f]* >20M (старше текущего прогона),
  /tmp/pa-comp-* /tmp/pa-worker-compact-* (остатки компакшн-e2e), чужие /tmp-кэши.
- Upstream-пакет готов: .research/upstream-reports/ (8 черновиков + README-индекс,
  вход через Discussion, отправка только по команде пользователя).
