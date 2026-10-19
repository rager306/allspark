# Verify: reliability-батч (#7, #8, #20, #21) — перепроверка ревью CE 2026-10-16

Верификатор: субагент verify-reliability умер на 24-й строке транскрипта; батч
доделан вручную (рутовая сессия). HEAD 375ad95cf, ветка feat/isolated-defaults.

## #7 [P1] «Anthropic wallet drain retries as transient fault» — CONFIRMED (с уточнением)

Механизм (весь проверен в коде):
- `classify_stream_failure` (pa-ai/src/utils_inner/stream_failure.rs:527-586) маппит
  платёжные сигналы ТОЛЬКО через `status == Some(402)`; тексты
  `insufficient_credits/balance/quota_exceeded/...` без статуса падают в `Unknown`.
- Три места СОЗНАТЕЛЬНО пинят `is_payment_signal → PaymentRequired` ВОКРУГ классификатора:
  1) `from_error_frame` (provider_errors.rs:377-380, комментарий «a drained wallet never
     refills inside a retry ladder»),
  2) общий HTTP-путь (stream_failure.rs:738-748),
  3) комментарий ссылается на то же правило для фреймов.
- **НО** Anthropic-SSE путь `anthropic_sse_error` (providers/anthropic/stream.rs:48-78)
  зовёт `classify_stream_failure(error_type, None)` НАПРЯМУЮ, без пина → in-stream
  error-фрейм с insufficient_credits из-за шлюза перед Anthropic классифицируется `Unknown`.
- Последствие подтверждено: `is_permanent_provider_failure_kind`
  (pa-core/src/session_engine/provider_retry.rs:141-160): `"payment_required" → true`
  (не ретраится, failover-цепочка перехватывает и рерутит), а `unknown → false` →
  полный transient-лэддер бессмысленных ретраев, failover НЕ срабатывает.

Уточнение формулировки: «wallet drain» — преувеличение (пустой кошелёк не тратится
ретраями); реальный дефект — пропуск сигнала на одном пути → нет рерута + пустой
лэддер ретраев. Severity P1 справедлив (несогласованность классификации = неверное
поверение failover). Фикс тривиален: применить тот же `is_payment_signal`-пин в
`anthropic_sse_error`.

## #8 [P1] «Session provider calls have no timeout when cancel signal present» — CONFIRMED (даже шире)

Механизм:
- `http.rs:253-261`: `match (&signal, request.timeout_ms)` — первая ветка
  `(Some(signal), _)` делает `select!(cancelled, send)` и **дискардит timeout_ms
  вайлдкардом**; ветка таймаута `(None, Some(_))` недостижима при наличии сигнала.
- `provider_adapter.rs:180`: сессионные ходы шлют `timeout_ms: None` + signal →
  ВСЕГДА первая ветка. Даже если бы таймаут задали — он бы игнорировался.
- SSE-чтения тела (`http.rs:67-86`): select(cancel, chunk) — без idle-границы:
  чёрная дыра посреди стрима висит до пользовательского abort.
- Дивергенция внутри репо подтверждена: bedrock `h2.rs:281-287` делает правильно
  (timeout поверх select_cancel) — общий путь http.rs расходится с образцом.
- reqwest-клиенты (`utils_inner/http.rs:17-41`): только `http1_only()` +
  `pool_idle_timeout(90s)` (это про пул коннектов, не про запрос); builder в tls.rs
  добавляет только корневые сертификаты. Никаких .timeout()/.connect_timeout().

Verdict: P1 справедлив. Фикс: единый `timeout(select_cancel(...))` в http.rs (образец
h2.rs) + задать timeout_ms в provider_adapter.

## #20 [P1] «Compaction freezes UI on two daemon RPCs» — CONFIRMED

- `interactive/run.rs:1332-1337`: при `transcript_stale` (компакция завершилась)
  главный UI-цикл INLINE ждёт `rebuild_transcript(...).await` →
  `refresh_stats(...).await` последовательно.
- `rebuild_transcript` (apply.rs:110-135) = bounded 10s RPC GetMessages;
  `refresh_stats` (lifecycle.rs:705-722) = bounded 10s RPC GetSessionStats.
  Итого до ~20с голодания input/spinner/quiet-tick/reconnect-рук на заклинившем
  воркере; тот же inline-паттерн на settle turn'а (10s).
- Ретроспектива не бесплатна, но ограничена (bounded 10s×2) — P1 на грани P2;
  оставляем P1 (это главный цикл UI, и паттерн повторяется).
- Фикс из ревью корректен: spawn + epoch guard + доставка каналом
  (образец `spawn_transcript_backfill` уже в коде).

## #21 [P1] «Run loop awaits telemetry before first paint» — PARTIAL (понижение до P2)

- INLINE await подтверждён: `run.rs:837` `telemetry.enhanced_keys(...).await` в теле
  run-loop до select!; соседний hyperlinks-путь (846-852) правильно спавнит, с
  комментарием, почему await нельзя — enhanced_keys выбивается из образца.
- НО сам future НЕ сетевой: `enhanced_keys` (telemetry.rs:260-272) =
  `with(counters…)` + `Box::pin(ready(()))` — мгновенно готов. Сетевого await нет.
- Реальная цена: `with()` → `telemetry_disabled(&self.settings())`, а `settings()`
  (telemetry.rs:42-44) строит НОВЫЙ `SettingsManager::create(cwd, agent_dir)` —
  синхронное чтение файлов настроек на UI-потоке при каждом вызове. На холодном/медленном
  диске это stall, но не network-round-trip и не first-paint-блокер класса #3288.
- Verdict: PARTIAL — паттерн-нарушение и лишний sync-IO реальны, формулировка ревью
  («awaits telemetry», «network») неточна. Понижаем до P2; фикс тривиален (спавнить как
  hyperlinks + кэшировать settings). Красный тест the_startup_flush… этот путь не
  покрывает — стоит расширить на «ни один telemetry-вызов не await'ится в run-loop».

## Сводка батча

| # | Вердикт | Severity | Первый шаг фикса |
|---|---|---|---|
| 7 | CONFIRMED (формулировка «drain» преувеличена) | P1 | is_payment_signal-пин в anthropic_sse_error |
| 8 | CONFIRMED (шире: таймаут дискардится при сигнале + None по умолчанию) | P1 | timeout(select_cancel()) по образцу h2.rs + timeout_ms в adapter |
| 20 | CONFIRMED (bounded 10s×2 inline в UI-цикле) | P1→P1/P2 | spawn + epoch guard по образцу spawn_transcript_backfill |
| 21 | PARTIAL (inline await реален, future готов; sync settings-IO реален) | P1→P2 | спавнить как hyperlinks; кэш settings; расширить красный тест |
