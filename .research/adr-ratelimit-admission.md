# ADR: Rate-limit-aware admission для субагентов (pa-daemon)

Статус: proposed → готов к реализации (волна 2). Дата: 2026-10-16.
Источники: инцидент 429 (9 параллельных субагентов zai/glm-5.3 → 3 смерти),
исходники oh-my-pi (/tmp/omp-src) и codex (/tmp/codex-src),
.research/ratelimit-best-practices.md (28 практик, 40 источников).

## Проблема
Retry/park/failover в форке — per-session. N субагентов независимо превышают
потолок провайдера: каждый прожигает свой 3-попыточный лэддер (~секунды) и
умирает, хотя лимит общий. Ровно oh-my-pi #5207.

## Дизайн (M1→M5 по приоритетам исследования)

### M1 (P0): кросс-сессийный AIMD-семафор per (provider, model)
- Место: новый модуль `pa-daemon/src/agent_engine/provider_admission.rs`;
  реестр в состоянии демона, общий для всех session-воркеров.
- Ключ: resolved (provider, model); стартовый лимит 6, floor 1, cap = settings.
- AIMD: успешный стрим → limit += 1 (медленно, до cap); 429 → limit = max(1, floor(limit/2))
  (быстро); окно восстановления — secondsSinceLast429.
- Узкий бракет (omp #3749): слот держится ТОЛЬКО на время одного LLM-стрима
  (acquire перед provider-вызовом, release на settle/error/abort), НЕ на всё
  время жизни воркера — иначе дедлок деревьев спавнов.
- Настройка: `[agents] providerConcurrency = { start = 6 }` (или без настройки —
  константы + телеметрия; решить при имплементации по минимуму поверхности).

### M2 (P0): bounded fan-out + spawn-queue со stagger
- Admission в `rlm.run` (pa-core/src/session_engine/rlm_host.rs:602 → bridge):
  спавн сверх текущего AIMD-лимита провайдера → очередь (FIFO), handle
  возвращается сразу (rlm_child_id), промпт доставляется при освобождении слота.
- Stagger/jitter диспатча: randomized 0–2с перед стартом воркера из очереди
  (thundering herd; replay-эксперимент: jitter+budget = 78.3% vs 0%).
- ВАЖНО: очередь живёт в демоне (кросс-процессно), не в py-фасаде; factory.py
  queue/max_parallel уже есть — не дублировать, а пробросить в демон.

### M3 (P1): typed RateLimitReason + Retry-After дисциплина
- Классы (omp-классификация, сверенная с практиками): QUOTA_EXHAUSTED (30мин,
  НЕ ретраить — acct-local, ротация/failover), RATE_LIMIT (30с, ретрай),
  CONCURRENT (5с), CAPACITY (45±15с), SERVER (20с), SPEND_CAP (без
  retry-after — не ретраить).
- Retry-After (RFC 9110 §10.2.3) = МИНИМУМ ожидания, cap 30–60с; max_attempts
  для 429 → 5–7 (сейчас 3 — причина смертей).
- Интеграция с существующим: pa-core provider_retry.rs (там уже jitter ±20%),
  error_classify; не ломать failover-цепочку (payment уже корректно рерутит).

### M4 (P1): retry-budget + circuit breaker по провайдеру
- Google SRE: per-request ≤3 попытки (есть), per-client budget ≤10% ретраев
  (в окне); превышение → breaker OPEN по провайдеру (новые ходы ждут/фолбэчат),
  half-open probe одним запросом.
- Учитывает существующие: total retries ≤8, park до reset, backpressure 128.

### M5 (P2): наблюдаемость
- В list_subagents/state: current_limit, in_flight, ceiling, 429-streak per
  provider; queue depth спавнов; provider_retry_outcome уже наружу есть.

## Чего НЕ делаем
- Токен-бакет по токенам (не по запросам) — отложить: у нас нет per-model
  token-норм провайдеров в каталоге; AIMD по запросам достаточно для кейса.
- Глобальный семафор в pa-core per-session — бесполезен при N процессах.

## Тесты
1. Red-first: 9 спавнов на провайдере с лимитом 6 → 3 в очереди, промпты
   доставлены после release; ни одна сессия не умирает от 429-шторма (mock).
2. AIMD: серия 429 → limit падает вдвое; серия успехов → растёт на 1 до cap.
3. Узкий бракет: слот освобождается на settle стрима (не на конец воркера) —
   вложенные спавны не дедлокаются.
4. Retry-After: 429 c hint 5с → ждёт 5с (не 30с), cap 60с; без hint → классовый
   бэкофф; QUOTA/SPEND_CAP → сразу failover-класс, не ретрай.
5. Очередь FIFO + stagger: порядок доставки сохранён, старты разнесены.

## Фазирование
Wave 2a: M1+M2 (демон, admission+queue+AIMD) — после завершения fix-provider
и fix-exactly-once (конфликтуют зонами pa-daemon/pa-core).
Wave 2b: M3+M4 (классификация+бюджет). Wave 2c: M5 (телеметрия) + readme-fork.
