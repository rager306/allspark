# prime-typesafe-jev (japorto100) — разбор и сравнение с нашим pa-jev

- **Дата**: 2026-10-17. Клон: `/tmp/prime-typesafe-jev`, HEAD `0a0b1a5e59bc32ed0639004e1b2c8dc02dc453d6`.
- **Происхождение фактов**: прочитан весь код (~6.4k строк TS+Python), тесты запущены и проходят:
  `node --test` → 164 теста (161 pass, 3 skipped, 0 fail), pytest → 24/24.

## 1. Что это

**НЕ форк prime-agent.** Отдельный MIT-проект: host-extension (`extension/`, TypeScript)
+ Python-skill (`skill/`) для **TS-эры Prime Agent v0.9.8+** — интеграция decision-модели
TypeSafe JEV (System One, `api.typesafe.ai/v1/systemone`) как «System-1»-слоя быстрых
калиброванных решений + жёсткое governance (безопасность, бюджет, приватность).

- Репозиторий: github.com/japorto100/prime-typesafe-jev, создан 2026-10-14, **1 коммит**,
  0 stars/forks. Автор коммита `japorto100`, в LICENSE/package.json — «lipfi» (одно лицо,
  судя по всему; комментарии в коде на немецком).
- Проект извлечён из приватной среды «prime-lab» (внутреннее имя пакета
  `prime-lab-typesafe-bridge`; README ссылается на `plan-archive/` и `archive/`, которых в
  репозитории нет — провенанс тонкий, истории нет).
- Авторский контекст: форк `japorto100/jev-harness` (питонья копия PromtEngineer/jev-harness,
  2026-09-29) и соседний `japorto100/prime-workflow` (та же схема extension+skill+promote) —
  автор просто коллекционирует/строит расширения к pi/prime.
- **Лицензия: MIT** (`LICENSE`, целиком стандартный текст, «Copyright (c) 2026 lipfi»):
  разрешает use/copy/modify/merge/publish/sublicense/sell при сохранении copyright-строки.
  Код можно брать с атрибуцией.
- Чему «верить» в бейджах: README заявляет «185 tests passing» — фактически 161+3 skipped
  (node) + 24 (pytest); суть верна.
- README указывает целевые точки встраивания: TS-extension API (`pi.on(...)`,
  `pi.registerCommand`) — событий `before_agent_start`, `session_start`, `tool_call`,
  `turn_end`, `tool_result`, `session_before_compact`, `session_before_refine`, `input`.
  **Наш Rust-форк такого extension-API не имеет** (grep по crates: нет), так что их код у нас
  не загрузится как есть — переносить паттерны/модули, а не пакет.

## 2. Архитектура (file:line)

```
Python-ядро (skill typesafe-jev)  ──UDS PRIME_JEV_SOCKET──▶  Host-extension (Node)
  system_one_async / choice / score / noul                   floor → redactor → budget →
  (skill/src/typesafe_jev/__init__.py:29)                    flow-guard → provider → validator
                                                             → receipts v2 + eval-channel
                                                                          │ HTTPS
                                                                          ▼ api.typesafe.ai/v1/systemone
```

- **Транспорт**: приватный UDS, mkdtemp 0700 + socket 0600 + `owner.json` с pid
  (`extension/transport.ts:43`); один запрос на соединение, лимиты 64 KiB req / 1 MiB resp /
  16 соединений / 5s socket-timeout. Синглтон через `Symbol.for(...)` живёт через reload
  расширения, `retire()` только по «quit» (`transport.ts:149`); sweep мёртвых мостов
  (мёртвый pid + возраст >1ч). **Ключи API остаются в host-процессе** — skill их не видит.
- **Provider-seam**: ровно один провайдер `jev` (`extension/core/provider.ts:57`), дефолт
  `model: jev-latest` (валидация регексом), общий дедлайн **1200 ms**
  (`provider.ts:5`), один ограниченный retry на 429/529 (Retry-After ≤400ms, иначе 150ms,
  `provider.ts:124`), capability-матрица (batching, maxQuestions=64, timeoutClass, egress,
  cost). `PRIME_JEV_PROVIDER` выбирает из Map — seam под второго провайдера заготовлен.
- **Egress-redactor** (`extension/core/redactor.ts:94`): структурно-сохраняющая редакция
  payload ДО сериализации (ключи выживают, значения редаются; sensitive-ключи всегда);
  правила: private keys, Bearer, prefixed tokens (sk-/pk-/ghp-/glpat-/npm-/xox…), путь-дени
  (.env*, auth.json, *.pem, .ssh/**), AKIA, `key=value`, URL-credentials. Режимы
  `PRIME_JEV_EGRESS`: `redact` (default) / `block` / `off`.
- **Бюджет** (`extension/core/budget.ts:67`, kill-switch `:302`): персистентный дневной
  файл `receipts/jev.budget.json`; fail-closed persistence (O_NOFOLLOW, валидация счётчиков,
  tempfile+fsync+rename, файловый лок со stale-cleanup, ревизии + перечитка против «украденного»
  лока); **резервации**: `reserve(hook,bytes)` → `send()` / `release()` (идемпотентные,
  привязаны к id; возврат при не-отправке); дефолты: 200 вызовов и 512 KiB на процесс,
  1000 вызовов / 5 MiB в день, пер-хуковые капы; skipped считаются отдельно; `PRIME_JEV_BUDGET_KILL=1`.
- **Flow-guard** (`extension/core/flow-guard.ts:27`): rolling-minute rate cap (60/мин,
  `PRIME_JEV_RATE_PER_MIN`) + circuit breaker (5 фейлов подряд → открыт 60с,
  `PRIME_JEV_BREAKER_*`); процесс-локально.
- **Детерминированный floor** (`extension/core/floor.ts:90`): двухуровневый (block/warn)
  регекс-трипваер для bash и ipython (rm -rf, mkfs, dd of=/dev/…, DROP TABLE, push -f,
  kill -9 1 / rmtree, chmod 777…), с маскировкой литералов/комментариев и понижением
  «упоминания dd» до warn. Честно задокументирован как tripwire, НЕ security-boundary.
- **Хуки** (все default `off`; лестница `off → G1 → shadow → advisory → live`,
  `extension/hooks/modes.ts:23`; G1 — явные env-гранты по паре hook×dataclass, `modes.ts:121`;
  настройки через `extensionConfig.jev.*` в settings.json через vendored-резолвер,
  env > settings > default, битые настройки ломаются громко):
  - **tool-gate** (`hooks/tool-gate.ts:131`): floor первым (block — жёстко, warn — HITL,
    без TTY fail-closed); затем JEV в серой зоне только для «mutating-выглядящих» разрешённых
    входов; 7 независимых risk-предикатов noul (`RISK_PREDICATES`, `tool-gate.ts:29`);
    дедуп-кэш на (session, tool, payload, cwd, questionsVersion); в live HITL только по
    `PRIME_JEV_TOOLGATE_CONFIRM_ON`; ошибка JEV = fail-open к floor.
  - **router** (`hooks/router.ts:509`): сначала детерминированный шортлист скиллов
    (IDF-взвешенный лексскоринг + DE→EN алиасы, `router.ts:404`), затем noul-гейты +
    `best_skill` choice с `none`-эскейпом (+опционально best_model); advisory-инъекция хинта
    только при явном пороге `PRIME_JEV_ROUTER_MIN_P` + маркер-идемпотентность; дедуп
    «ситуации решения».
  - **watchdog** (`turn_end`, `hooks/watchdog.ts:148`): turn-facts (ошибки, каденция каждые 3
    хода, хвост-эксерпт 512B + digest), 6 noul-сигналов прогресса; асинхронно (concurrency 1),
    подсказка видима только если ход ещё актуален.
  - **result-triage** (`tool_result`, `hooks/result-triage.ts:123`): только результаты >32 KiB;
    дедуп по краям-хэшу; пер-сессионная история сигнатур (`recent_evidence_signatures`);
    очередь с нагрузочным контрактом (maxPending 32 / 8 MiB / 60s / coalescing) — контент
    никогда не мутируется.
  - **compaction** (`session_before_compact`, `hooks/compaction.ts:191`): shadow-only,
    ВСЕГДА возвращает undefined; ≤8 блоков с retention-signal предранжированием; по одному
    `preserve_<id>` noul на блок; уважает host-abort.
  - **refine-advisor** (`session_before_refine`, `hooks/refine-advisor.ts:104`): shadow-only,
    6 noul о «reuse-worthiness» раунда рефайнмента; планировщик не трогает.
  - **input-triage** (opt-in): классификация CONTROL/EPHEMERAL/AMENDMENT — только UI-хинт.
- **Async-инфраструктура**: job-раннер с нагрузочным контрактом и coalescing
  (`hooks/async-lifecycle.ts:62`), привязка задач к (session, run-epoch) с инвалидацией
  чужих эпох (`hooks/run-context.ts:38`).
- **Наблюдаемость**: receipts `prime-jev/call/2` — content-free (без промптов/ответов),
  v1/v2 union-reader (`extension/receipts.ts:57`); отдельный eval-канал `prime-jev/eval/1`
  — только типизированные предсказания, идентификаторы санитизятся в безопасные или
  непрозрачные хэши (`core/eval-channel.ts:12`); версионированные контракты вопросов —
  digest от набора вопросов как якорь поколения калибровки (`core/question-registry.ts:32`).
  Поля `would`/`would_action` — «что сделал бы live-гейт» — измеряются в shadow.
- **Валидация ответов** (`core/validator.ts:28`, `distribution.ts:11`): distribution-консистентность
  в интервалах округления (jev-1.13.0 округляет до сотых), choice обязан быть (связанным)
  аргмаксом, score — взвешенное значение (может быть дробным), legend обязан совпасть с рубрикой.
- **Клиент-skills**: `system_one_async` (настоящий async через to_thread; sync-обёртки
  choice/score/noul «притворяются awaitable» и честно это документируют), никакого
  mock-фолбэка — нет моста = `JevBridgeError`; legacy-фолбэки помечены и различимы через
  `result()` (status answered|fallback + provenance). `coding_questions.py` — рецепты
  (patch_risk, evidence_sufficiency, per-requirement noul, next_test, role_selection,
  source_selection). Промпт-политика для системного промпта — идемпотентный append
  `coding-policy.ts:4`.
- **Промоут** (`scripts/promote.mjs:48`): copy-only установка в `~/.prime/agent/{extensions,
  skills}` с SHA-256 манифестами, бэкапами и receipt; dry-run/--apply/--check.
- **Eval-харнесс** (`eval/router-eval.mjs`): headless hit@1 по курируемым router-задачам
  (train/holdout/holdout2), жёсткие правила разметки (`eval/LABELS.md`): abstention-задачи
  считаются отдельно от hit@1, без name-leak.

## 3. Сравнение с нашим pa-jev (Phase 1 + Phase 2 в работе)

| Ось | Они (prime-typesafe-jev) | Мы (pa-jev + pa-core::jev) | Кто сильнее |
|---|---|---|---|
| Форма интеграции | Внешняя TS-extension на pi-extension-API (TS v0.9.8+), UDS-мост, skill отдельно | Встроено в движок: крейт `pa-jev` + `pa-core::jev`, нативный тула `jev`, ноль IPC | Мы (глубже); они переносимее на TS-продукт |
| Провайдеры | 1 (typesafe), seam под второго; дефолт `jev-latest` | 4 wire-диалекта (typesafe, openrouter-decisions, workers-ai, openai-decisions), цепочка кандидатов из каталога моделей, native-only правило | Мы |
| Отказ/деградация | Timeout 1.2s общий, 1 retry на 429/529, circuit breaker 5→60s, kill-switch | Retry 3× (500ms→5s), кулдаун 401/402/403 5мин по identity, chunking ≤4, abort/timeout без фолбэка, answer-cache | Мы по цепочке; **у них breaker + kill-switch — у нас нет** |
| Приватность/egress | Структурная редакция до сериализации, режимы redact/block/off, path-deny | Нет редакции (ADR молчит) | **Они** |
| Бюджет/лимиты | Персистентный дневной бюджет, резервации+возвраты, per-hook капы, rate/min | Только answer-cache + экономика в ADR (~$0.00002/решение) | **Они** |
| Consent/роллаут | Лестница off→G1→shadow→advisory→live на каждый хук; G1-гранты данных; битые настройки = громкая ошибка | `[jev] enabled off|auto|on` + corpus-гейты (Phase 2 eval/), правило ADR «shadow неделю» без машинерии | **Они** (машинерия); фаза-2 гейты у нас уже глубже в плане |
| Наблюдаемость | content-free receipts v2 + типизированный eval-канал + версии контрактов вопросов + would/would_action | usage → model_usage леджер + telemetry span `judgment` | **Они** (измерение), мы (учёт затрат) |
| Валидация ответов | Интервалы округления, choice≡argmax, score-взвешенность, legend-совпадение | Wire-тесты против doc-fixtures (ADR: score = weighted index и т.д.) | Примерно равно; их интервальная математика точнее описана |
| Потребители | router (скилл-роутинг), tool-gate, watchdog, result-triage, compaction curation, refine-advisor, input-triage | JevTool (ask/classify/route/check); Phase 2: auto-thinking, graph re-rank; Phase 3: kernel judge(), компакция, edit-review | Они шире по хукам; у нас глубже интеграция с движком (thinking-уровни, граф) |
| Безопасность тулов | Детерминированный floor + JEV-серая зона, HITL, fail-closed без TTY | ADR сознательно: Judge НИКОГДА в edit-guards/provider-failover; hard rules первыми | Разные задачи; их floor — самостоятельный паттерн |
| Кэш | пер-хук дедуп (ситуация/сессия), без answer-cache | answer-cache (Phase 1; SQLite по ADR) | Мы |
| State-гигиена | excerpt ≤4 KiB/1 KiB/512B + sha256-digest заголовок при обрезке, 64 KiB на запрос | ADR правило 7: truncate до ~32k Jev-токенов | **Они** (digest-провенанс) |
| Сессии/асинхронность | (session, run-epoch)-биндинг, очередь с контрактом нагрузки, coalescing | неактуально (in-process), но паттерн нужен для Phase 2/3 потребителей | Они (переиспользуемо) |
| Тесты | 161+24, оффлайн, adversarial-корпус floor | wire/chunk/retry/cache/gating тесты по ADR + corpus-гейты | сопоставимо |

**Совместимость подходов: высокая.** Общие инварианты совпадают почти дословно (оба
наследуют конвенции TypeSafe и, видимо, oh-my-pi-паттерны; их README цитирует
`@mohan-cao/pi-jev-response-router` и `pi-jev-guard` как blueprints): hard rules первыми;
суждение = предложение, не авторизация; явный `none`/`unknown`-эскейп; батчить вопросы;
«недоступен ≠ отрицательный ответ»; fail-open advisory / fail-closed side-effects; shadow
до live; версии вопросов как поколение калибровки. Расхождения: они — governance-first и
advisory-only поверх TS-хостов; мы — engine-first с цепочкой диалектов и переключением
thinking-уровня. Противоречий с ADR нет, кроме двух мелочей (см. риски).

## 4. Вердикт и рекомендации

### (a) Паттерны, которые стоит перенести

1. **Лестница роллаута `off → G1 → shadow → advisory → live` на каждого потребителя** с
   env-only грантами данных. Прямое усиление нашего `[jev] enabled` для Phase 2/3: каждая
   фича (auto-thinking, re-rank, tool arbiter, компакция) получает свою ступень и
   shadow-метрику `would_action` до включения. Закрывает ADR-правило 4 машинерией.
2. **Egress-redactor в клиенте pa-jev** (шаг prepare, до сериализации; sensitive-ключи
   всегда; redact/block/off). Дешёво (~150 строк Rust) и закрывает реальную дыру: state
   уходит на сторонний endpoint.
3. **Дневной бюджет + kill-switch** (`[jev] budget`: dayCalls/dayBytes + `PRIME_JEV_BUDGET_KILL`
   -аналог в settings/env). Резервации с возвратом — по желанию; для нашего объёма хватит
   простых счётчиков с персистентностью, но reserve/send/release-семантика у них образцовая.
4. **Circuit breaker поверх цепочки** (5 фейлов → 60с пауза на endpoint): у нас кулдаун
   срабатывает только на 401/402/403 конкретного кандидата; общий breaker спасает, когда
   ВСЕ upstream-ы лежат (и не жечь retry-бюджет).
5. **Content-free receipts + eval-канал + versions контрактов вопросов**: у нас corpus-гейты
   (eval/) уже есть — добавить jsonl-receipt `hook/mode/outcome/latency/would_action` и digest
   набора вопросов как поле калибровки. Пороги без pinned generation несравнимы (их аргумент).
6. **Нагрузочный контракт асинхронных потребителей** (bounded queue, bytes, deadline,
   per-session coalescing, (session, run-epoch)-биндинг) — обязательный каркас для Phase 2/3,
   чтобы judge-потребители никогда не задерживали ход/paint (наш же red-first тест
   критического пути).
7. **Excerpt + digest-заголовок** при обрезке state (ADR правило 7 + провенанс).
8. **Интервальная валидация ответов** (rounding intervals, choice≡argmax-with-ties,
   legend≡rubric) — добавить в wire-валидацию pa-jev.
9. **Refine-advisor как кандидат Phase 3**: noul-фильтр «reuse-worthiness» перед записью в
   continual harness — прямо обслуживает наше правило «memories must be kept lean».
10. **Router-паттерн «детерминированный шортлист → judge choice с none»** — применимо к
    нашему tool arbiter (Phase 2) и подбору скиллов: код строит меню из IDF-шортлиста,
    judge указывает; у нас это уже ADR-правило 2, но их IDF-шортлист — готовый рецепт.

### (b) Код, который можно взять (MIT, с атрибуцией «prime-typesafe-jev, MIT, japorto100/lipfi»)

- `extension/core/redactor.ts` (117 строк) → порт в Rust в pa-jev (простые регексы).
- `extension/core/flow-guard.ts` (66 строк) → breaker/rate в pa-jev client или chain.
- `extension/core/budget.ts` (373 строки) → при желании полный бюджет; иначе взять схему
  файла+лока.
- `extension/distribution.ts` + `core/validator.ts` (~150 строк суммарно) → интервальная
  валидация ответов.
- `skill/src/typesafe_jev/coding_questions.py` (161 строка, чистый Python) → рецепты в
  документацию/skill к JevTool почти как есть.
- `eval/LABELS.md` правила разметки → наш corpus-gate (abstention отдельно от hit@1, без
  name-leak, версионирование меток).
- НЕ брать: floor.ts (у нас нет tool-gate и ADR его не планирует; если появится — их
  двухуровневая схема + mention-tier — хороший чертёж), transport.ts (наш движок in-process;
  если делаем Phase 3 `judge()` для Python-REPL — их UDS-мост это готовый прецедент дизайна:
  ключи в host, ядро без кредов, один мост на reload).

### (c) Риски и конфликты с нашим ADR

- **Среда исполнения**: их код несовместим с нашим бинарником (нет extension-API в Rust
  порту); перенос = порт паттернов, не установка. На TS-продукте upstream их работа живёт
  сама по себе.
- **`jev-latest` по умолчанию** (provider.ts:57) против нашего пина `jev-1.13.0` (jagged
  edges, пороги калибруются под версию). Держим наш пин; при контакте с автором — указать.
- **Дедлайн 1200 ms** против нашего таймаута 10s: их цифра для advisory-хуков на быстром
  System-1; наши 10s покрывают медленные диалекты. Не конфликт, но для неблокирующих
  потребителей стоит рассмотреть отдельный короткий advisory-таймаут.
- **Германские UI-строки и комментарии** — при порте переводить; user-visible поверхности
  нашего форка — Prime Agent English + наш брандинг.
- **Провенанс**: 1 коммит, ссылается на отсутствующие archive/ — код зрелый (следы длинной
  ревью-истории в комментариях: «Verify-Fix 24.09», «Audit 26.09»), но истории нет;
  рассчитывать только на текущее состояние.
- Изоляция форка: их пути `~/.prime/agent` — у нас HOME-изолированный `~/.allspark-home`,
  всё наследуется корректно; env `PRIME_JEV_*` не конфликтуют с нашими `TYPESAFE_*`/`[jev]`.

### (d) Кооперация с автором

Низкий приоритет, но вежливо и полезно: repo 3 дня, 0 звёзд, личная выжимка. Варианты:
1. Issue «спасибо + у нас Rust-порт pa-jev с диалектами/цепочкой; ваши governance-паттерны
   переносим с атрибуцией» — дешёвый сигнал, возможный обмен идеями.
2. PR им: наши wire-диалекты (openrouter-decisions, openai-decisions трансляция) ложатся в
   их provider-seam как вторые провайдеры — но объём работы > ценности для нас.
3. Отдельно интересно: их breaker/budget идеи в апстрим-обсуждения oh-my-pi (там живая
   экосистема judgment-слоёв).
Рекомендация: **не кооперироваться активно сейчас**; взять паттерны/код по MIT с атрибуцией
и поставить watch на репозиторий. Вернуться к контакту, если наш Phase 3 (`judge()` host-request
для REPL) будет делаться — их UDS-мост тогда лучший референс.

## 5. Резюме в одну строку

Их проект — «governance-обвязка JEV для TS-эры prime-agent» (UDS-мост, floor, redactor,
бюджет, G1-лестница, receipts/eval, 7 advisory-хуков); наш — «engine-встроенный judge-слой
с цепочкой диалектов». Подходы ортогональны и совместимы: у нас сильнее провайдер-слой и
интеграция, у них — приватность, бюджет, consent-роллаут и измерение. Переносим краслайны
(redactor, breaker+budget, receipts/eval, лестница роллаута, интервальная валидация,
async-контракт) — код MIT, атрибуция обязательна.
