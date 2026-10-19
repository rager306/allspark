# Лучшие практики обработки rate-limit (429) и ограничения конкурентности для LLM-агентов с параллельными субагентами

Источник: веб-исследование (Tavily/Exa, 17 поисковых запросов + 25 extract) в контексте фикса Rust-форка
prime-agent (allspark). Дата: 2026-10-16. Ветка: `feat/isolated-defaults`, HEAD `375ad95cf`.

Расширяет (не дублирует) уже известное:
- **oh-my-pi**: per-provider semaphore вокруг КАЖДОГО LLM-запроса (узкий бракет), resize-in-place,
  классификация `RateLimitReason`, ротация креденшлов.
- **codex**: `RetryPolicy{max_attempts, base 200ms ×2, retry_on 429/5xx/transport}`, jitter ±10%,
  `Retry-After` из заголовков, телеметрия, аккаунт-снапшоты лимитов наружу.

Терминологическая поправка к постановке: 429 определён в **RFC 6585 §4**, а `Retry-After` — в
**RFC 9110 §10.2.3** (не 7807/8594: RFC 7807 — `application/problem+json`, RFC 8594 — заголовок
`Sunset`). Ни 429, ни `Retry-After` не задают единого «scope» лимита — его определяет сам API.

---

## 1. Таблица практик (с источниками)

| # | Практика | Суть | Источник (URL) |
|---|----------|------|----------------|
| P1 | **AIMD-конкурентность для LLM API** | Адаптивный предел in-flight запросов: медленный additive increase, быстрый multiplicative decrease по сигналу 429. Асимметрия намеренна: «be optimistic cautiously, be pessimistic decisively». Старт от `max_parallel_requests`, опционально `rampup_seconds` (линейный разгон с 1). | NVIDIA NeMo Data Designer — https://docs.nvidia.com/nemo/datadesigner/dev-notes/owning-the-model-stack |
| P2 | **Разделить retry-транспорт и throttle-адаптацию** | 502/503/504 и transport-ошибки ретраит транспорт; **429 НЕ глотать** — пробросить наверх в throttle-менеджер, чтобы он вызвал `release_rate_limited()` (срезать лимит, cooldown, записать ceiling). Смешение = retry-шторм. | NVIDIA NeMo — https://docs.nvidia.com/nemo/datadesigner/dev-notes/owning-the-model-stack |
| P3 | **Throttle-домены** | Состояние AIMD keyed by `(provider_name, model_id, throttle_domain)` (`chat`/`embedding`/`image`/`healthcheck`): `current_limit`, `in_flight`, `blocked_until`, `success_streak`, `rate_limit_ceiling`. Всплеск 429 на chat не должен морить embedding. | NVIDIA NeMo — https://docs.nvidia.com/nemo/datadesigner/dev-notes/owning-the-model-stack |
| P4 | **Netflix concurrency-limits: Vegas/Gradient/AIMD** | Клиент обычно `AIMDLimit`, сервер `VegasLimit` (`L ≈ limit·(1 − minRTT/sampleRTT)`); `Gradient2` сглаживает выбросы двумя EMA (long/short). Инвариант Литтла: `Limit = AverageRPS × AverageLatency`. | https://github.com/Netflix/concurrency-limits |
| P5 | **Concurrency > rate-limit при переменной длительности** | Rate-limit (RPS) не адаптируется к длительности запроса: 100 простых = 10k RPS, но 100 «тяжёлых» по 100 мс = 1k RPS. Concurrency-лимит пропорционален реальному потреблению ресурса. | https://www.techinterview.org/post/3233473210/lld-adaptive-concurrency-limiting |
| P6 | **Concurrency-лимит нужен отдельно от RPM/TPM** | «Клиент шлёт 10 req/s, каждый живёт минуту → ~600 открытых запросов»; per-minute quota это не остановит. Нужен max-in-flight + **bounded queue + admission timeout**; долгий стрим держит слот до завершения/отмены. Anthropic предупреждает: per-minute лимит может применяться на коротких интервалах → burst падает при «безопасном» среднем. | WSO2 — https://wso2.com/api-platform/learn/llm-rate-limiting |
| P7 | **Token-bucket: burst vs sustained** | Bucket отделяет capacity (размер burst) от refill rate (устойчивая скорость). Anthropic использует token-bucket. Реальные реализации: оценка input при admission + settle реального usage после ответа. | https://www.getmaxim.ai/articles/rate-limiting ; https://mlflow.org/articles/rate-limiting-llm ; https://agentgateway.dev/docs/kubernetes/latest/documentation/llm/rate-limit |
| P8 | **429 ≠ 5xx: различать quota и overload** | 429 обычно = свой лимит/квота; 529/5xx = провайдер перегружен (другая реакция; долбить ретраями хуже для всех). | https://mlflow.org/articles/rate-limiting-llm |
| P9 | **Full jitter > equal/decorrelated** | AWS: no-jitter — явный проигравший (и по работе, и по времени); Full Jitter — меньше работы; Decorrelated Jitter близко. | https://aws.amazon.com/blogs/architecture/exponential-backoff-and-jitter/ |
| P10 | **Jitter внутри агент-фреймворка, а не только в инфре** | Инфра-бэкофф не помогает от синхронного завершения параллельных субагентов: нужен randomized stagger диспатча субагентов + jittered TTL на прогрев кэша. | https://dataplatformadvisory.com/blog/2026/08/26/ai-agent-fanout-thundering-herd-database-resiliency |
| P11 | **Агентный thundering herd — реальность** | Datadog (Feb 2026): rate-limit ошибки = **60% всех LLM-span-ошибок** (~8.4M/мес); агенты ретраят весь multi-step workflow от последнего чекпоинта → синхронный re-run тысяч сессий. Cockroach нашёл точку перегиба **700–1000 агентов**. | https://www.cockroachlabs.com/blog/agentic-ai-thundering-herd-problem |
| P12 | **Replay трёх стратегий 429** | Immediate retry: 0% успеха; synchronized backoff: упорядоченные всплески (тоже 0%); jitter + total budget: **78.3%** успеха. | https://nbility.ai/blog/en/429-retry-storm |
| P13 | **`Retry-After` — минимум, не максимум** | Трактовать как минимум, добавлять небольшой random; OpenAI: `slow_down` (429) → follow Retry-After, снизить rate, потом плавно поднимать; `server_is_overloaded` (503) → follow, при повторе увеличивать задержку. | https://developers.openai.com/api/docs/guides/rate-limits |
| P14 | **Cap на wait + max attempts** | Max delay 30–60 с; max attempts 5–7; после — вернуть ошибку наверх. Не ретраить сразу, не ретраить параллельно, не спайкать после восстановления, не полагаться на фиксированное окно. | https://developer.docebo.com/docs/best-practices-for-handling-api-rate-limits-and-429-errors |
| P15 | **Партиционировать backoff по throttle-scope** | 429 может применяться к key/tenant/resource/region/endpoint/service. Не ставить на паузу всех из-за одного tenant; keyить shared state по документированному scope, ограничивать число ключей; бизнес-максимум «устаревания» работы → dead-letter. Retryability и idempotency — раздельно. | https://oneuptime.com/blog/post/2026-08-14-retry-after-vs-exponential-backoff/view |
| P16 | **Spend-cap 429 без `Retry-After`** | Anthropic: `enforced_spend_limit_reached` — тот же `rate_limit_error`, но **без** `retry-after`; ретраи (включая авто в SDK) бесполезны до снятия лимита. Отличать от транзиентного 429. | https://docs.claude.com/en/api/rate-limits |
| P17 | **Rate-limit заголовки наружу** | Anthropic: `retry-after`, `anthropic-ratelimit-requests/tokens/input-tokens/output-tokens-{limit,remaining,reset}` (значения — для самого строгого действующего лимита, per-model). OpenAI: org-уровень RPM/TPM + project-scoped token limits. | https://docs.claude.com/en/api/rate-limits ; https://developers.openai.com/api/docs/guides/rate-limits |
| P18 | **Retry budget (per-client, ~10%)** | Google SRE: per-request бюджет 3 попытки; per-client бюджет — ретраи ≤ **10%** от запросов; иначе fail fast. Плюс adaptive throttling: клиент сам себя тормозит, когда доля reject высока (`requests`/`accepts` за 2 мин). | https://sre.google/sre-book/handling-overload |
| P19 | **Централизовать throttling на слое очереди** | Temporal: лимит на Task Queue (`max_task_queue_activities_per_second`) держит кап при любом числе воркеров; связанные паттерны — Priority / Fairness / Worker-Specific Queues. НО: это throughput, а не concurrency-лимит. | https://docs.temporal.io/design-patterns/downstream-rate-limiting |
| P20 | **Admission/backpressure с типизированным отказом** | Bounded in-flight + bounded queue; при насыщении — типизированный `overloaded` (не молчаливый дроп); «saturated worker answers with typed refusal; internal routes wait, never refuse». | https://www.nilus.be/blog/event-driven_backpressure_patterns_in_reactive_microservices |
| P21 | **Bounded fan-out и max_depth** | Codex multi-agent: `max_threads = 6`, `max_depth = 1` (ребёнок может спавнить, глубже — нет); повышение depth → «repeated fan-out, token/latency/resource blowup». | https://www.firecrawl.dev/blog/codex-multi-agent-orchestration |
| P22 | **Circuit breaker (closed/open/half-open)** | После порога подряд-фейлов — open, fail fast на fallback, дать провайдеру восстановиться; через recovery timeout — half-open, один probe. Гейтить только транзиентные статусы (не считать 400/401 — иначе misconfig гонит весь трафик на fallback). Осторожно с идемпотентностью при fallback. | https://www.sitepoint.com/claude-api-circuit-breaker-pattern |
| P23 | **Failover по статус-кодам** | 429 → мгновенно рероутить на вторичного провайдера; 5xx → ретрай на другом провайдере. «Automatic failover обычно лучше retries»; retries — когда провайдер обязан быть фиксированным. | https://portkey.ai/blog/failover-routing-strategies-for-llms-in-production ; https://docs.helicone.ai/features/advanced-usage/retries |
| P24 | **Стриминг — самый сложный failover** | Мид-стрим обрыв: token-perfect resume невозможен (нет «continue from here»); retry = генерация с нуля → **двойной биллинг** (400 токенов + 400 снова). Минимизировать cap, логировать каждый retry. | https://ai-tldr.dev/learn/llm-apis/streaming-structured-outputs/handle-streaming-errors ; https://www.truefoundry.com/blog/llm-failover-load-balancing-provider-outages |
| P25 | **Secondary rate limits (GitHub-style)** | 403/429; concurrency ≤ **100**; ≤900 points/min REST (2000 GraphQL); при `retry-after` — ждать столько, при `x-ratelimit-remaining: 0` — до `x-ratelimit-reset`, иначе ≥1 мин; далее экспонента + лимит попыток; **не** строить логику на точном остатке. | https://docs.github.com/rest/using-the-rest-api/rate-limits-for-the-rest-api |
| P26 | **Fair scheduling для task-parallel агентов** | Justitia (arXiv 2510.17015): VTC/FairServe; жёсткое fair-sharing тормозит end-to-end JCT против монополизации → «selective pampering». | https://arxiv.org/html/2510.17015v2 |
| P27 | **Гейтвей как единая точка enforcement** | LiteLLM router: `fallbacks` (в т.ч. `RateLimitError`), `rpm` на деплой, трекинг fallback в spend-логах. Portkey: circuit breaker, LB между API-ключами против rate-limit, budget/rate limits. TrueFoundry: fallback-правила по 429/5xx, per-user/team/model. agentgateway: `Retry-After` = refill-delay для локального лимита, longest reset для глобального (default 30 с). | https://docs.litellm.ai/docs/proxy/reliability ; https://docs.portkey.ai/docs/product/ai-gateway ; https://www.truefoundry.com/blog/rate-limiting-in-llm-gateway ; https://agentgateway.dev/docs/kubernetes/latest/documentation/llm/rate-limit |
| P28 | **oh-my-pi: узкий бракет — не дедлок** | Issue #3749: semaphore, удерживаемый всё время жизни агента, → circular wait деревьев спавнов. Фикс PR #3751: перенести в `streamFn`-обёртку, слот держится ТОЛЬКО на один LLM-запрос. Issue #3812: per-model semaphore keyed by resolved `provider/model`, release когда **стрим завершился**; `task.maxConcurrency` — внешний/глобальный кап. Issue #5207: кросс-процессный per-provider/model лимит; `task.maxConcurrency` недостаточен (advisor ест ёмкость независимо; инстансы не координируются). | https://github.com/can1357/oh-my-pi/issues/3749 ; https://github.com/can1357/oh-my-pi/issues/3812 ; https://github.com/can1357/oh-my-pi/issues/5207 |

---

## 2. Что применимо к нашему кейсу

**Наш кейс:** родительский агент спавнит N субагентов-воркеров; **каждый субагент — своя сессия со своим
LLM-стримом** (в prime-agent: отдельный воркер под супервизором `pa-daemon`, свой session-engine, свой
HTTP-стрим); провайдер отдаёт 429 при **~9 параллельных** запросах.

### 2.1 Ключевое противоречие нашего кейса

Провайдер считает конкурентность на своей стороне (org/model-scope), а у нас **N независимых сессий**,
которые друг о друге не знают. Значит:

- **Per-session лимит бесполезен**: каждая сессия честно держит «свой» лимит, но суммарно мы превышаем
  провайдерский потолок ровно так, как описано в oh-my-pi #5207 (инстансы не координируются).
- **Лимит «на всё время жизни субагента» = дедлок**: субагент может спавнить своих субагентов
  (#3749). Слот обязан жить **только вокруг одного LLM-запроса** (стрима), освобождаться на settle.
- **Статический семафор ≠ достаточно**: knee ~9 не константа — зависит от длины промпта (TPM), модели,
  времени суток, общего трафика аккаунта. Нужен **адаптивный** предел (AIMD), а не «9 навсегда».

Отсюда три независимых слоя защиты (все три нужны, они дополняют друг друга):

```
родитель (admission)          →  bounded fan-out: max_parallel, spawn queue, stagger
   │  spawn
   ▼
daemon (global provider cap)  →  ОБЩИЙ per-provider/model семафор + AIMD, кросс-сессийный
   │  LLM request
   ▼
сессия (retry/park/failover)  →  429-таксономия, Retry-After, jitter, park, failover
```

### 2.2 Что у нас уже есть (по коду HEAD `375ad95cf`)

| Механика | Где | Состояние |
|---|---|---|
| `ProviderRetryPolicy{max_retries:3, base_delay_ms:2000, max_retry_delay_ms:60000, max_delay_ms:unbounded}` + **jitter ±20%** (`RETRY_JITTER_FRACTION=0.2`) | `pa-core/src/session_engine/provider_retry.rs` | ✅ есть, jitter уже санационированная дивергенция от TS |
| `ProviderParkPolicy` — парковка сессии до reset (`pauseUntilReset`, grace 30 с, max_parks 8, max_pause 1 день) | `pa-core/src/session_engine/provider_park.rs` | ✅ есть (аналог quota-exhausted) |
| `ProviderFailoverPolicy` — 3 retry/провайдер, 1 с ×2 cap 30 с, **total ≤ 8**, ротация кандидатов | `pa-core/src/session_engine/provider_failover.rs` | ✅ есть |
| `StreamFailureKind::RateLimit` (regex `rate_limit\|usage_limit\|throttl` или 429), `Overloaded` (529) | `pa-ai/src/utils_inner/stream_failure.rs` | ✅ типизация есть, но **без** под-классов (quota vs concurrent vs capacity) |
| Backpressure воркера: `WORKER_INFLIGHT_CAPACITY=128`, типизированный `worker_overloaded` | `pa-daemon/src/backpressure.rs` | ✅ есть (это про RPC, не про LLM-провайдера) |
| Factory spawn admission: `max_parallel` (default 8, cap 64), `max_children` (default 10000), backoff по строке-маркеру (`BACKOFF_BASE=1.0`, cap 60 с, max 5 попыток), `admission_backoff_until`, «rate-limited admissions defer to the loop» | `prime-agent-runtime/src/rlm/factory.py` | ⚠️ частично: **нет jitter**, детект 429 — строковый эвристик (`_RATE_LIMIT_MARKERS`), нет typed-сигнала |
| RLM ledger (spawn/rename/delete admissions) | `pa-daemon/src/rlm_ledger.rs` | ✅ есть — точка врезки admission-метрик |

### 2.3 Чего не хватает (пробелы под фикс)

1. **Нет ОБЩЕГО (кросс-сессийного) per-provider/model LLM-семафора.** Ретай/парк/failover — per-session;
   при N субагентах они независимо бьют в один провайдер. Это ровно oh-my-pi #5207.
2. **Нет AIMD** — предел статичен (`max_parallel=8`, но knee ~9 при коротких промптах и ниже при длинных).
3. **Нет typed `RateLimitReason`** — только общий `RateLimit`; нельзя отличить `QUOTA_EXHAUSTED`
   (парковать) от `CONCURRENT`/`CAPACITY` (короткий cooldown) от `SERVER` (транспортный retry).
4. **Jitter отсутствует в factory spawn-backoff** (в provider_retry он есть) → синхронные волны спавна.
5. **Нет retry-budget** (доля ретраев ≤10%) и **нет circuit breaker** по провайдеру — при устойчивом 429
   каждый субагент независимо долбит провайдера.
6. **Нет парсинга rate-limit заголовков** (`retry-after`, `anthropic-ratelimit-*`, `x-ratelimit-*`) наружу
   — нельзя тормозить ДО отказа (proactive headroom).

---

## 3. Конкретные механики с рекомендацией приоритета

### M1 — P0. Глобальный per-provider/model LLM-семафор с AIMD (узкий бракет)

**Что:** один общий лимитер на `(provider, model[, domain])`, живущий в супервизоре `pa-daemon`
(кросс-сессийный и кросс-процессный), а не в каждой сессии. Слот берётся **перед каждым LLM-запросом**
и освобождается **на settle стрима** — никогда на всё время жизни субагента (#3749).

**Параметры (стартовая точка, не догма):**
- `current_limit` стартует ниже knee, напр. **6** (наблюдаемый knee ~9 — оставить запас), с опциональным
  `rampup_seconds` при холодном старте;
- **multiplicative decrease** на 429: `limit ← max(floor, floor(limit × 0.5))`, cooldown, запись
  `rate_limit_ceiling` (не подниматься выше наблюдённого провала сразу);
- **additive increase**: `+1` за каждые `success_streak ≥ K` успешных стримов (K≈10–20), только вне cooldown;
- `floor = 1` (гарантия прогресса), `cap = max_parallel` родителя.
- 429 **не глотать** в retry-слое — пробрасывать наверх в лимитер (P2).

**Почему P0:** это единственная механика, которая чинит корневую причину (независимые сессии превышают
общий потолок) и адаптируется к плавающему knee. Всё остальное — про то, как дороже/дешевле пережить 429.

**Врезка в наш код:** `pa-daemon` уже владеет жизненным циклом воркеров и `rlm_ledger` — лимитер встаёт
рядом (например, `pa-daemon/src/provider_limiter.rs`), а `pa-ai`-стрим оборачивается acquire/release
(аналог `wrapStreamFnWithProviderConcurrency`). Настройки — в `retry.provider.*` (уже есть группа).

---

### M2 — P0. Bounded fan-out + spawn queue со stagger/jitter

**Что:** родительский агент **не** спавнит N субагентов разом. Очередь допускает по одному на
освобождение слота (это в `factory.py::_spawn_ready` уже есть: «admit pending instances up to
`max_parallel`»), плюс:
- **randomized stagger** на диспатч (P10): `delay = random(0, J)` перед каждым admission, чтобы
  синхронные старты не били волной;
- **jitter в spawn-backoff**: `factory.py` сейчас пишет `BACKOFF_BASE × 2^(streak−1)` без jitter —
  добавить ±20% (как в `provider_retry::jittered_delay_ms`), иначе все factory-ноды ретраят admission
  синхронно (P9/P12: full jitter критичен);
- `max_depth` — держать 1 (P21), иначе «repeated fan-out».

**Почему P0:** это дёшево (частично реализовано) и прямо снимает thundering herd на старте; без него M1
всё равно получит синхронный залп из N сессий, и первый же 429 обрушит `current_limit` неоптимально.

---

### M3 — P1. Typed `RateLimitReason` + дисциплина `Retry-After`

**Что:** расщепить `StreamFailureKind::RateLimit` на под-классы (расширяя уже известную oh-my-pi-таксономию):

| Reason | Признак | Реакция | Wait |
|---|---|---|---|
| `QUOTA_EXHAUSTED` | reset далеко (часы), `usage_limit`, spend-cap | **park** сессии до reset (`provider_park`) | до reset, cap 1 день, max 8/episode |
| `RATE_LIMIT` | 429 + `retry-after` короткий | retry с jitter, honor `Retry-After` как **минимум** | ~30 с |
| `CONCURRENT` | превышен in-flight | **срезать `current_limit`** (M1), короткий retry | ~5 с |
| `CAPACITY` | 429/529 перегрузка модели | backoff + возможный failover | 45 ± 15 с |
| `SERVER` | 5xx/transport | транспортный retry (не throttle) | 20 с |
| `SPEND_CAP` | 429 **без** `retry-after` | **не ретраить**, surfacing наружу | — |

Плюс: `Retry-After` трактовать как минимум (P13), cap на wait (30–60 с, P14), `max_attempts` 5–7,
партиционировать по scope (P15). Заголовки (`retry-after`, `anthropic-ratelimit-*`, `x-ratelimit-*`)
парсить и **поверхность наружу** (P17) для proactive throttling.

**Почему P1:** превращает «слепой retry» в правильную реакцию; без него M1 не знает, *почему* пришёл 429,
и может парковать то, что надо было лишь притормозить (или наоборот).

---

### M4 — P1. Retry-budget + per-provider circuit breaker

**Что:**
- **Retry-budget** (P18): доля ретраев ≤ **10%** от запросов в скользящем окне (2 мин); при превышении —
  fail fast. Per-episode cap на общее число ретраев (у нас уже `MAX_TOTAL_PROVIDER_RETRIES=8` — оставить,
  добавить глобальный бюджет).
- **Circuit breaker** (P22): после `threshold` подряд-фейлов одного провайдера → `open` → сразу failover
  (P23), не ждать; через cooldown → `half-open`, один probe. Гейтить **только** транзиентные статусы
  (429/5xx/transport; не 400/401/402 — иначе misconfig/дренаж кошелька гонит трафик на fallback, что у нас
  уже частично закрыто `is_permanent_provider_failure_kind`).
- **Стриминг** (P24): мид-стрим обрыв — не «resume», а «regenerate from start»; считать двойной биллинг,
  не ретраить бесконечно, логировать.

**Почему P1:** защищает от каскада, когда 429 устойчив; M1/M2 разгрузят провайдера, а M4 не даст N сессиям
независимо долбить его в cooldown.

---

### M5 — P2. Наблюдаемость и headroom-телеметрия

**Что:** вывести наружу (в UI/статус, как codex account-snapshots): `current_limit`, `in_flight`,
`blocked_until`, `rate_limit_ceiling`, `success_streak` (M1); retry-телеметрию (attempt/reason/wait);
парсинг заголовков remaining/reset (P17); события `spawn_backoff` уже есть в `factory.py` — добавить
reason и wait. Метрики: доля 429, доля ретраев (retry-budget), глубина spawn-queue, время в park.

**Почему P2:** без наблюдаемости AIMD-параметры и knee не настроить; но это не блокер запуска.

---

### Сводная приоритизация

| Приоритет | Механика | Ось проблемы | Стоимость |
|---|---|---|---|
| **P0** | M1 — глобальный per-provider/model AIMD-семафор (узкий бракет) | корневая причина (кросс-сессийный потолок) | средняя |
| **P0** | M2 — bounded fan-out + spawn queue со stagger/jitter | thundering herd на старте | низкая (частично есть) |
| **P1** | M3 — typed `RateLimitReason` + `Retry-After`-дисциплина | правильность реакции | средняя |
| **P1** | M4 — retry-budget + circuit breaker + failover | каскад при устойчивом 429 | средняя |
| **P2** | M5 — наблюдаемость/headroom | настройка и доверие | низкая |

**Порядок внедрения:** M2 (быстро, снимает пик) → M1 (ядро) → M3 (питает M1 корректным сигналом) →
M4 (защита) → M5 (настройка). Инвариант на всех слоях: **бракет узкий** (один LLM-стрим), **429 не
глотать**, **jitter везде**, **Retry-After — минимум**, **failover вместо бесконечного ожидания**.

---

## Приложение: полный список источников

1. https://docs.nvidia.com/nemo/datadesigner/dev-notes/owning-the-model-stack — AIMD для LLM API, throttle-домены, retry boundary
2. https://github.com/Netflix/concurrency-limits — Vegas/Gradient/AIMD
3. https://www.techinterview.org/post/3233473210/lld-adaptive-concurrency-limiting — concurrency vs rate
4. https://wso2.com/api-platform/learn/llm-rate-limiting — token bucket + max-in-flight + bounded queue
5. https://www.getmaxim.ai/articles/rate-limiting — token bucket, Anthropic
6. https://mlflow.org/articles/rate-limiting-llm — RPM/TPM/RPD, 429 vs 529, full jitter
7. https://agentgateway.dev/docs/kubernetes/latest/documentation/llm/rate-limit — Retry-After = refill delay
8. https://www.truefoundry.com/blog/rate-limiting-in-llm-gateway — fallback-правила, per-user/team/model
9. https://docs.portkey.ai/docs/product/ai-gateway — circuit breaker, LB между ключами
10. https://docs.litellm.ai/docs/proxy/reliability — fallbacks, rpm, spend logs
11. https://developers.cloudflare.com/ai-gateway — request retry & fallback
12. https://docs.helicone.ai/features/advanced-usage/retries — failover vs retries
13. https://aws.amazon.com/blogs/architecture/exponential-backoff-and-jitter/ — full/equal/decorrelated jitter
14. https://www.cockroachlabs.com/blog/agentic-ai-thundering-herd-problem — агентный herd, checkpoint re-run
15. https://dataplatformadvisory.com/blog/2026/08/26/ai-agent-fanout-thundering-herd-database-resiliency — jitter внутри фреймворка, 700–1000 агентов
16. https://nbility.ai/blog/en/429-retry-storm — replay трёх стратегий
17. https://developer.docebo.com/docs/best-practices-for-handling-api-rate-limits-and-429-errors — schedule, caps
18. https://oneuptime.com/blog/post/2026-08-14-retry-after-vs-exponential-backoff/view — throttle scope, dead-letter
19. https://developers.openai.com/api/docs/guides/rate-limits — slow_down/server_is_overloaded
20. https://docs.claude.com/en/api/rate-limits — заголовки, spend-cap 429
21. https://platform.claude.com/docs/en/api/rate-limits — batch/managed agents/files лимиты
22. https://hidekazu-konishi.com/entry/anthropic_claude_api_errors_reference.html — 429 vs 529, диагностика
23. https://ai.google.dev/gemini-api/docs/troubleshooting — Gemini 429
24. https://docs.github.com/rest/using-the-rest-api/rate-limits-for-the-rest-api — secondary rate limits
25. https://sre.google/sre-book/handling-overload — retry budget, adaptive throttling
26. https://docs.temporal.io/design-patterns/downstream-rate-limiting — Task Queue throttling, priority/fairness
27. https://www.nilus.be/blog/event-driven_backpressure_patterns_in_reactive_microservices — admission control
28. https://www.firecrawl.dev/blog/codex-multi-agent-orchestration — max_threads/max_depth
29. https://www.sitepoint.com/claude-api-circuit-breaker-pattern — circuit breaker
30. https://portkey.ai/blog/failover-routing-strategies-for-llms-in-production — failover по статус-кодам
31. https://www.truefoundry.com/blog/llm-failover-load-balancing-provider-outages — streaming failover hard case
32. https://ai-tldr.dev/learn/llm-apis/streaming-structured-outputs/handle-streaming-errors — mid-stream, double billing
33. https://arxiv.org/html/2510.17015v2 — Justitia, fair scheduling
34. https://www.getmaxim.ai/articles/how-ai-gateways-tackle-rate-limiting-for-llm-apps — key pooling, headroom
35. https://github.com/can1357/oh-my-pi/issues/3749 — deadlock lifetime-scoped semaphore
36. https://github.com/can1357/oh-my-pi/issues/3812 — per-model semaphore, release on stream finish
37. https://github.com/can1357/oh-my-pi/issues/5207 — кросс-процессный per-provider/model лимит
38. https://sparkco.ai/blog/mastering-retry-logic-agents-a-deep-dive-into-2025-best-practices — transient vs permanent
39. https://huggingface.co/blog/tngtech/llm-performance-request-queueing — request queueing
40. https://medium.com/@avnein4988/mitigating-the-thundering-herd-problem-exponential-backoff-with-jitter-b507cdf90d62 — jitter
