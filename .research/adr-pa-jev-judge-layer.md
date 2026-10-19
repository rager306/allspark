# ADR: слой Judge (`pa-jev`) — типизированные решения в форке allspark

- **Статус**: proposed (зафиксирован 2026-10-16, сессия по итогам исследования jev)
- **Источники**: `.research/jev-deep-research.md`, `.research/jev-fork-integration.md`,
  `.research/arsumbris-jev-research.md`, исходники oh-my-pi `/tmp/omp-src` (все факты
  ниже с точностью до файла/константы сверены с кодом), заметки vault `knowledge/jev*.md`.

## Контекст

Jev (TypeSafe System One, релиз 2026-09-15) — decision-модель: `state` + типизированные
вопросы → калиброванные вероятности. Примитивы: Choice (≤255 опций + probabilities +
confidence), Score (шкала 2–10), Noul (да/нет, P∈0..1). $0.042/MTok input, output
бесплатен; реальное решение ~$0.00002, 140–500 ms. Независимые замеры: vs LLM-judge —
равная accuracy при ~300× дешевле / ~23× быстрее; на уверенных ответах (p>0.9/<0.1) —
ноль ошибок на 125 items; судьи ошибаются в одних местах (96% корреляции ошибок) —
каскад экономит деньги, но не поднимает потолок точности.

Форк имеет дорогие потребители суждений (нативные тулы lsp/graph/edit, компакция,
thinking-уровни, граф-поиск) и не имеет дешёвого калиброванного слоя решений.
Требования пользователя: (1) возможность выключить; (2) корректная деградация при
недоступности jev; (3) совместимость с другими SDK-совместимыми decision-моделями
(пример — модель Microsoft) без правок клиента.

## Решение (кратко)

Слой Judge по образцу oh-my-pi: один трейт `Judge`, классификация бэкендов **по
wire-диалекту API, не по провайдеру**, role-chain кандидатов с правилом native-only,
кулдауны отвергнутых аккаунтов, answer-cache, гейтинг фич `auto`. Новый крейт
`crates/pa-jev` + модуль `pa-core::jev` + настройки `[jev]` **default OFF**.

## Архитектура

```
pa-jev (NEW crate, #![forbid(unsafe_code)])
    wire-типы (Question/Answer field-for-field с API) + JudgmentWire (диалекты)
    JevClient: chunking ≤4 в полёте, retry 3× (backoff 500ms→5s, retry-after),
    timeout 10s, Usage { cost = input/1e6 * 0.042 }
    ^
pa-core::jev (NEW mod)
    trait Judge; ChainJudge (лестница деградации); answer-cache (SQLite, rusqlite
    уже в workspace через pa-graph); JevTool (AgentTool: ask|classify|route|check);
    has_native_judge(settings, registry) -> bool
    ^
pa-core::session_engine / pa-daemon
    create_session wiring, native_tool_names, usage → model_usage леджер + /cache
    ^
потребители (Phase 2+): auto-thinking, graph re-rank, tool arbiter, kernel judge(),
compaction curation
```

## Логика деградации (лестница)

```
0. jev.enabled = "off"     → слой выключен: consumers идут детерминированным путём, $0
1. hard rules              → решает код, Judge не зовётся (перед любым суждением)
2. native chain (по порядку):
     typesafe/jev-1.13.0 → openrouter/jev-1.13.0 → microsoft/<model> → …
3. native-кандидатов нет   → если jev.fallback = "llm": prompted-судья на smol-роли
                             (default "off": лучше выключить фичу, чем некалиброванные
                             пороги; auto-гейт сам погасит judge-тяжёлых потребителей)
4. кандидат упал:
     401/402/403  → кулдаун 5 мин (keyed по identity модели), следующий кандидат
     429/408/5xx  → 3 retry внутри клиента → следующий кандидат
     нет ключа    → кандидат пропускается
     abort/timeout→ наружу сразу, БЕЗ фолбэка (отмена не должна тратить других)
     все упали    → ошибка потребителю; fail-open/fail-closed решает потребитель:
                    advisory (edit-review, кэш-аннотации) = fail-open,
                    side-effecting гейты = fail-closed (эскалация человеку)
```

Правило **native-only**: после первого native-кандидата prompted-кандидаты из цепочки
исключаются. Prompted-модель не воспроизведёт калиброванные вероятности — подмена
тихо ломает пороги (0.7/0.9). Обратное допустимо: если native нет совсем и
`fallback = "llm"` — сессия может подставить свою активную модель как prompted-судью
(`withSessionFallback` в omp).

## Совместимость по SDK: диалекты wire

Один `JevClient`, несколько `JudgmentWire`-диалектов (omp: `JUDGMENT_WIRES`, native.ts):

| Диалект | Транспорт | Тело |
|---|---|---|
| `typesafe` | `POST {base}/v1/systemone` | как есть |
| `openrouter-decisions` | `POST {base}/api/alpha/decisions` | то же тело |
| `workers-ai-systemone` | `POST {base}/ai/run/{model}` | System One в envelope Cloudflare |
| `openai-decisions` | `POST {base}/decisions` | трансляция в обе стороны |

Трансляция `openai-decisions` (omp openai-decisions.ts, образец для новых диалектов):
`state→input`; `noul`+criteria→`predicate` (criteria вшиваются в instructions); `choice`
criteria→`choices: [{value, description}]`; `score` levels→`levels: [{label}]` lowest
first; `noul`-ответ→`probability`; probabilities→список `[{value, probability}]`;
`refusal`-ответ = отказ от вопроса.

**Добавление модели Microsoft = запись в каталог моделей форка** (`models_fork`):
`api: <диалект>` + `baseUrl` + `judgment-questions-max` + цена. Если endpoint совместим
с OpenAI Decisions или System One — ноль кода; свой envelope — новый `JudgmentWire`
(~200 строк, по образцу openai-decisions). Классификация всегда по диалекту, никогда
`provider == "microsoft"`.

Env-совместимость с официальным SDK (omp native.ts): `TYPESAFE_API_KEY`,
`TYPESAFE_BASE_URL`, `TYPESAFE_DEFAULT_MODEL` — резолвятся через auth-registry форка.

## Ссылочная реализация oh-my-pi (факты из исходников)

Файлы: `packages/ai/src/judgment/{native,systemone,openai-decisions,wire,types,text,chat}.ts`,
`packages/coding-agent/src/judgment/index.ts`, `test/judgment-chain.test.ts`.

- `kindOf`: native (`isJudgmentApi(model.api)`) | local (`api === "local-inference"`,
  on-device sub-2B, keyword-ответ, max 16 токенов / 1024 для reasoning) | online
  (prompted chat через шаблон text-judge.md, парс `<id>: <answer>`).
- `judgeRoleChain`: `resolveRoleChain("judge", …)`; от первого native — только native.
- `ChainJudge.withCandidate`: обход кандидатов; 401/402/403 → `CANDIDATE_REJECTION_
  COOLDOWN_MS = 5 мин` (Map на registry, keyed по `formatModelStringWithRouting`);
  нет ключа → skip; abort/timeout → rethrow; все упали → `judgment: every judge
  candidate failed`. Потребитель может задавать разные вопросы разным kind'ам
  (local — coarse bucket, online — точный tier).
- Константы клиента: `DEFAULT_TIMEOUT_MS = 10_000`, `MAX_ATTEMPTS = 3`,
  `BACKOFF_BASE_MS = 500`, `BACKOFF_MAX_MS = 5_000`, `CHUNK_PARALLEL = 4`.
- Chunking: >`judgment-questions-max` вопросов → чанки, ≤4 параллельно; первый упавший
  чанк абортит siblings (doomed judgment не тратит); usage выживших — в
  `onPartialUsage` перед reject.
- Кэши: answer-cache SQLite (ключ = canonical state + model + name + type +
  instruction + criteria; наружу только pending-вопросы; полностью из кэша = $0 и
  без записи в леджер); chain-cache 1s TTL (`CANDIDATE_TTL_MS`).
- Атрибуция: каждая попытка ровно один раз — леджер сессии + telemetry span
  `judgment`; цена OpenRouter сохраняется, иначе price из каталога.
- Гейтинг: `find.enabled: "auto"` включает find только когда `hasNativeJudge()`
  (первый кандидат цепочки — native). Legacy-миграция настроек собирает judge-chain
  из старых ключей (judgmentProvider/autoThinkingModel/unexpectedStopModel).
- Ответ API содержит точную ответившую версию модели — логировать (jagged edges:
  пороги калибруются под версию, пиновать `jev-1.13.0`, не `jev-latest`).

## Настройки форка

```toml
[jev]
enabled = "off"            # off | auto | on; default OFF — платный сторонний endpoint
                           # auto = judge-тяжёлые потребители активны только при native-судье
model = "typesafe/jev-1.13.0"        # пин версии (не jev-latest)
fallbackChain = []         # ["openrouter/jev-1.13.0", "microsoft/<model>"]
fallback = "off"           # off | llm — prompted-подмена только явная
timeoutMs = 10000
[jev.cache]
enabled = true             # SQLite answer-cache
```

Регистрация в `KNOWN_FIELDS` + getter рядом с `get_lsp_enabled`; wiring в
`create_session` (gate до move settings, push тула после `edit`, запись в
`native_tool_names`). В отличие от lsp/graph/edit (default ON) — default OFF.

## Правила (неизменяемые)

1. Hard rules первыми; Judge видит только серую зону.
2. Код строит меню, Judge указывает; всегда явная опция `none`/`other`.
3. Батчить вопросы одним запросом (state биллится один раз).
4. Пороги: act > 0.9, deny < 0.1, между — эскалация; тюнить на своём трафике
   (shadow-режим неделю до включения).
5. Fail-open (advisory) / fail-closed (side-effects) — решает потребитель.
6. Арифметику/даты — только код.
7. Truncate state (omp режет до 32k Jev-токенов / ~24k символов в git-stage).
8. Не менять главную модель mid-session (prompt-cache).
9. Baseline-тест против константы/coin-flip; polarity-flip тест вопросов.
10. Native-only после первого native (см. выше).

## Точки встраивания (фазы)

- **Phase 1**: `pa-jev` + `pa-core::jev` (`Judge`, `ChainJudge`, answer-cache,
  `JevTool`) + настройки + wiring + тесты. Без потребителей.
- **Phase 2** (по одному): auto-thinking difficulty classifier (цепочка подтверждена
  графом: `AgentSession::set_model_and_thinking_level` pa-core session_engine/mod.rs:351,
  `Worker::handle_set_thinking_level` pa-daemon/model_switch.rs:143); Jev re-rank top-K
  над pa-graph (FTS5/BM25/RRF/MMR уже есть); tool-selection arbiter над
  native_tool_names (pa-daemon/engine.rs:201).
- **Phase 3** (опции): `judge()` host-request для Python-REPL; компакция
  (`session_engine/compaction.rs` calculate_context_tokens:42) — какие tool-результаты
  живут; edit-review annotation (advisory); кэш-аннотации в `/cache`.
- **Явно НЕ делаем**: Judge внутри provider failover (`run_turn_with_provider_failover`)
  и внутри edit guards (seen-lines/stale-tag, pa-edit/src/session.rs:364) — там
  детерминизм первичен.

## Тесты (минимум)

- Wire encode/decode всех 3 примитивов против doc-fixtures (score = weighted index,
  probabilities по индексам, noul без confidence); оба диалекта (typesafe,
  openai-decisions) на одних фикстурах.
- Chunking: 40 вопросов при cap 16 → 3 запроса, merge, sum usage; первый чанк падает →
  siblings абортятся, onPartialUsage выживших.
- Retry: 429 c retry-after, 5xx — ретраятся; 400/404/422 — нет; 401/402/403 → кулдаун
  5 мин + следующий кандидат; все кандидаты упали → типизированная ошибка.
- Native-only: prompted-кандидат после native не вызывается никогда.
- Кэш: повторный вопрос к тому же state — 0 наружу; частично кэшированный — наружу
  только pending; полный кэш-хит не пишет usage в леджер.
- Гейтинг: `enabled=auto` без native-судьи → consumer детерминированный путь.
- Abort: судья не фолбэчится, отмена наружу.
- Красный тест критического пути: зависший jev-endpoint не задерживает paint/turn
  (fire-and-forget/timeout, по образцу the_startup_flush_never_blocks_the_first_frame).

## Последствия

- Экономика: ~$0.00002/решение; при ~500 решениях/день < 1¢; answer-cache обнуляет
  повторы.
- Риски: jagged edges версии (лечится пином), пороги (лечатся shadow-режимом),
  зависимость от стороннего endpoint (лечится цепочкой + default OFF + диалектной
  совместимостью — Microsoft и любые Decisions-совместимые).
- Upstream-чистота: всё в новом крейте + форк-маркировка (readme-fork.md), поверхность
  upstream не меняется.

## Phase 2 (реализовано): потребители + корпус-гейт

### Корпус-гейт (`pa-core/src/jev/eval/`)

Шаблон, которому обязан подчиняться каждый потребитель до включения по умолчанию
(урок RuFlo ADR-391: +0.89 п. на реальном корпусе не прошло гейт).

| Часть | Где | Контракт |
|---|---|---|
| `Corpus` / `Case` | `eval/mod.rs` | замороженный корпус: id + input + слепая метка + сплит; `fingerprint()` = sha256 по `id\|split\|label\|input` (тест фиксирует хеш) |
| `unseal_test()` | `eval/mod.rs` | одно касание test на инстанс корпуса; второй вызов — `EvalError::TestAlreadyTouched` |
| `TunedThresholds::tune` | `eval/mod.rs` | принимает **только dev**-отчёты (`WrongSplit`), вычитает safety margin, выбирает максимум из grid; тай не даёт порогов вовсе |
| `gate` | `eval/mod.rs` | принимает **только test**; отклоняет tie, суб-пороговую победу (`+0.9 п. < 1.0 п.`), латентность > `max_p95_latency_ratio` |
| `ScriptedJudge` | `eval/scripted.rs` | детерминированный маппинг state→ответы, сеть не зовётся |
| корпуса | `eval/corpus.rs` | `auto-thinking` 28 кейсов (dev 16 / test 12), `graph-rerank` 26 кейсов (dev 14 / test 12) |

Структурные инварианты (валидируются в `Corpus::new`, не в документации):
уникальные id, оба сплита непусты, **каждая test-метка присутствует в dev** (иначе
пороги нечем тюнить — корпус не создаётся).

Результаты на scripted-судье (проверяют проводку, не качество живой модели):

| Корпус | dev baseline → candidate | test baseline → candidate | Δ test | вердикт |
|---|---|---|---|---|
| auto-thinking | 0.375 → 1.000 | 0.417 → 1.000 | **+58.33 п.** | Promote (порог 5.0) |
| graph-rerank | 0.357 → 1.000 | 0.333 → 1.000 | **+66.67 п.** | Promote (порог 5.0) |

Baseline — сегодняшнее поведение форка (auto-thinking: детерминированный
`moderate`→`medium`; re-rank: RRF-порядок, top-1). Δ такой величины означает лишь, что
проводка (гейт → запрос → судья → разбор → уровень) работает; включение по умолчанию
требует замера на живом судье с тем же fingerprint. Тесты гейта: победа → Promote;
tie → Reject; `+0.9 п.` → Reject; `+1.0 п.` → Promote; рост p95 ×5.85 → Reject
(RuFlo MiniLM); полярно-перевёрнутый судья → проигрывает baseline и не тюнится.

### Потребитель 1: auto-thinking (`jev/auto_thinking.rs` + `session_engine/auto_thinking.rs`)

Один `choice` {trivial|moderate|hard|unsure} над препроцессированным промптом
(ANSI/фенсы вырезаны, схлопнуты пробелы, середина усечена до 2000 символов) →
`low|medium|high`. Врезка: `AgentSession::apply_auto_thinking` в
`prompt_with_images` (admission, до `agent.prompt`), уровень ставится через
`set_thinking_level` (та же книга, что `/thinking`).

- Гейт: `[jev] enabled = on|auto` **и** `[jev] autoThinking = auto` **и** native-цепочка.
  Default OFF; неизвестное значение настройки читается как off (fail-closed на опечатку).
- Fail-open: ошибка/таймаут/`unsure`/модель без reasoning → детерминированный уровень,
  ход идёт дальше. Таймаут 4 s (тест на paused-time доказывает срабатывание).
- `unsure` — ответ, а не сбой: не инкрементит счётчик. Два подряд сбоя → потребитель
  сам suspends на сессию (сломанная опциональная зависимость не должна облагать налогом
  каждый ход).

### Потребитель 2: graph re-rank (`jev/graph_rerank.rs` + `tools/graph.rs`)

Один запрос `noul` на кандидата по top-K ≤ 8 (`state` = query + список символов,
биллится один раз). Демотируется только `p < 0.1`; порядок выживших — исходный RRF
(судья фильтрует, не переупорядочивает; ничего не добавляет). Хвост за пределами top-K
не трогается и не помечается verified. Маркер `verified_jev` в `details` + поля
`verified`/`judged`/`relevance` на строке + суффикс `verified-by-jev` /
`demoted-by-jev (p=…)` в тексте. Fail-open: нет судьи/ошибка → результат поиска
байт-в-байт как раньше. Врезка — `GraphTool::rerank_search` на async-пути `execute`
(store-работа остаётся в `spawn_blocking`; минимальный diff, па-даемон не тронут).

### Потребитель 3: tool arbiter (`jev/tool_arbiter.rs` + `tools/bash.rs` + `tools/ipython.rs`)

Два слоя, оба advisory. **Слой 1 — детерминированная таблица** `RULES`
(порт omp `bashInterceptor.patterns`): `grep|rg|ag|ack` → `graph.search` (или
`lsp.find_symbol`, если graph выключен), `find|fd` → `graph.search`,
`sed -i|perl -i|awk -i inplace` → `edit`, `cat|head|tail` → `edit`. Правило
срабатывает только в **первичном сегменте** команды (до первого `|`/`;`/`&&`),
только в командной позиции (не `echo grep`, не `sudo rg`), не при
перенаправленном stdin (`grep x < file`) и не при veto (`tail -f`). Никогда не
предлагается тул, которого нет в ростере (`ToolAvailability` из
`active_tool_names`). **Слой 2 — суждение**: один `choice`
`use_specialized|use_bash|unsure` над state {command, shell_tool,
specialized_tool, reason}; `use_specialized` с confidence > 0.8 усиливает
формулировку. **Никогда не блокирует и не подменяет**: команда исполняется как
написана, exit-код решает успех, нота `[tool-arbiter] … nothing was blocked;
the call ran unchanged` дописывается ПОСЛЕ вывода + `details.toolArbiter`.
Врезка — в `execute_bash` (успешный путь) и в `execute_ipython` (по
`bashCommands.first`). Fail-open: нет судьи/ошибка/timeout → нота таблицы;
два подряд отказа судьи → слой суждения замолкает на сессию (таблица работает).

### Что осталось

- tool-arbiter: живой замер на судье по своему корпусу (`eval/corpus.rs::
  arbiter_corpus`, 15 кейсов, scripted-прогон зелёный, fingerprint запинен).
  `[jev] toolArbiter` по умолчанию `auto`, но весь арбитр (и таблица) стоит за
  `[jev] enabled` — у пользователя, не включившего слой, результат bash
  байт-в-байт прежний; слой суждения дополнительно требует native-цепочку.
- SQLite answer-cache (Phase 3, сейчас in-memory), компакция, edit-review, `judge()`
  host-request для Python-REPL.
- Замер на живом судье по обоим корпусам (scripted-числа выше — только проводка).
