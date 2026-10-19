# RuFlo × Jev: скан упоминаний (вместо субагента ruflo-jev-scan, умер без отчёта)

Дата: 2026-10-16. Метод: shallow clone ruvnet/ruflo → /tmp/ruflo-src, HEAD 1882006
(2026-10-14, v3.56.10, merge PR #4189). Grep по дереву: код, docs, ADR, конфиги MCP,
README, бенчмарки. Ключевые слова: jev, typesafe, systemone, "system one", system-one,
openjev, kev, rlcd, noul, decision, judgment, judge.

## 1. Буквальные упоминания «jev» — НЕТ (одно ложное срабатывание)

- `plugins/ruflo-swarm/tests/ruos.test.ts:91` — строка `'<ESC>[2Jevil<BEL>'` в тесте
  ANSI-санитизации («evil», не «jev»).
- Остальные «jev»-хиты — package-lock.json (base64-шум хешей).
- `systemone` / `system-one` / `system one` / `openjev` / `rlcd` — ноль попаданий.

## 2. НО: контракт Jev интегрирован через `@ruvector/typesafe` (OpenJev из RuVector PR #1029)

**Прямое попадание — `v3/@claude-flow/cli/src/ruvector/typesafe-router.ts`** — opt-in
аугментация `hooks_route` (выбор агента swarm) пакететом `@ruvector/typesafe@^0.1.0`
(optional peerDependency, package.json:146,153). Это локальная реализация Jev-контракта
(`decide(state, questions)`, `choice` с критериями `{what, not_for, examples}`,
probabilities + confidence + **abstain** + `calibrated`-флаг) — тот самый OpenJev
(bge-small 33M, weights в RuVector-репо, см. vault `openjev-ruvector-pr-1029`).

Правила интеграции (ADR-150 MetaHarness AND-gate, все четыре соблюдены):
1. **Removable** — dynamic import; любая ошибка загрузки/конструирования/decide →
   фолбэк на legacy-роутер.
2. **Opt-in** — ничего не делает без `CLAUDE_FLOW_ROUTER_TYPESAFE=1`; без флага пакет
   вообще не импортируется, legacy-результат возвращается как есть (тот же объект).
3. **Optional** — optional peer dependency.
4. **Honest** — результат несёт `routedBy`, confidence, abstain-mass и `calibrated`
   verbatim; hash-embedder по умолчанию некалиброван → `confidenceCalibrated: false`,
   и его confidence НИКОГДА не копируется в `estimatedMetrics.successProbability`.

**Гейты принятия ответа** (все должны держаться, иначе legacy-пик + `typesafe.reason`):
- `abstain <= maxAbstain` (default 0.30);
- **lift** = top-1 probability × options >= `minLift` (default 1.2 = 20% выше шанса) —
  lift, а не raw confidence, потому что confidence масштабируется с числом опций
  (~0.1 при 10 агентах) и зависит от embedder'а;
- top-1 обгоняет второго на >= `minMargin` (default 0.005); равномерное распределение
  (текст ни к чему не подходит) имеет margin 0.

Embedder: `'hash'` (default, некалиброван) или ONNX-каталог (bge-small/MiniLM) + manifest.

Точки поверхности: `hooks-tools.ts` (`withTypesafeRouting` — обёртка над legacy
handler'ом hooks_route), `doctor.ts --component typesafe` (health-check: установлен?
включён? какой embedder; подсказка `npm install @ruvector/typesafe`),
`docs/USERGUIDE.md:4524` (пользовательская дока).

## 3. Решающее: ADR-391 — бенчмарк-гейт, и типсаф НЕ прошёл

`v3/docs/adr/ADR-391-router-benchmark-gate.md` (2026-09-23, Implemented 3.44.0,
«first run: no candidate promoted») + бенчмарк
`v3/@claude-flow/cli/benchmarks/router/results/router-bench-2026-09-23.json`
(corpusSha256 b0c1923b…, corpus 197 промптов: dev 84 / test 113, с «ловушками» и
негативами, размечен вслепую, сплиты заморожены хешем; dev — тюнинг порогов,
test — решение, касание одного раза):

| Кандидат | test acc | macroF1 | p50 / p95, ms | Гейт |
|---|---|---|---|---|
| **A** текущий (hash+keyword) | 0.2566 | 0.1775 | 0.66 / **1.06** | default (остаётся) |
| B MiniLM real embedder (ADR-390) | 0.3628 | 0.3335 | 2.19 / 6.21 (+485%) | FAIL latency |
| **C** typesafe-hash (@ruvector/typesafe) | 0.2655 | 0.1992 | 0.77 / 1.20 | FAIL accuracy (+0.89 п.) |
| **D** typesafe-onnx (bge-small) | 0.2920 | 0.2269 | 6.07 / 8.35 (+686%) | FAIL latency |

Вывод RuFlo: **ни один кандидат не повышен** — дефолт остался hash-роутером;
typesafe-роутер живёт как opt-in. Порог лифта 1.2× сам TypeSafe ставил по выборке из
10 задач — RuFlo это явно называет недостаточным основанием менять дефолт.

Смежное: ADR-390 — семантический роутер на реальном sentence-embedder (MiniLM) тоже
opt-in: точность +10.6 п., но p95 ×5.85 — гейт латентности провален.

## 4. RuFlo ↔ RuVector (общая интеграция, одним разделом)

`v3/@claude-flow/cli/src/ruvector/` — 28 модулей: model-router (Thompson-sampling
Beta-Bernoulli bandit, cost-adjusted rewards, circuit breaker, priors в
.swarm/model-router-state.json), q-learning-router, neural-router, coverage-router,
semantic-router, router-calibrator, diff-classifier, graph-*, ast-analyzer, wasm-
embedder/ruvllm-wasm/agent-wasm, lora-adapter, vector-db и др. Зависимости:
`ruvector ^0.2.27`, `@ruvector/rabitq-wasm 0.1.0`, `@ruvector/typesafe ^0.1.0` (opt).
«noul»-хиты в model-router — это «Ber-noul-li» (Thompson sampling), не примитив Jev.

## 5. Выводы для форка allspark (pa-jev)

1. **Прецедент интеграции Jev-контракта существует и локально** (без TypeSafe API):
   @ruvector/typesafe = OpenJev как npm-пакет, decide()-контракт 1:1. Для pa-jev это
   вариант self-hosted native-кандидата цепочки Judge (диалект `ruvector-typesafe`,
   локальный, $0, offline) — ровно то, что ADR pa-jev называл «Phase 3, опция».
2. **Практика гейтов принимай-или-фолбэк переносится дословно**: abstain-масса, lift
   (не raw confidence — он масштабируется числом опций), margin, причина отказа в
   результате (`typesafe.reason`). Это готовый дизайн для `Judge`-ответов в pa-jev.
3. **Честность результата** (`routedBy`, `calibrated`, confidence никогда не
   подменяет метрику) — та же дисциплина, что наше правило «кэш/статистику не
   переписывать суждением».
4. **Главный эмпирический урок (в тон vault-заметкам)**: на реальном корпусе из 197
   задач локальный typesafe-роутер дал +0.89…+3.5 п. accuracy против тупого baseline и
   провалил гейт. Меняй дефолт ТОЛЬКО по измеренной победе на замороженном корпусе с
   вслепую размеченным test-сплитом. Для pa-jev это значит: каждый потребитель
   (auto-thinking, re-rank, arbiter) обязан иметь свой мини-корпус + гейт до
   включения по умолчанию; `enabled = "auto"` в ADR — правильный дефолт.
5. Opt-in/removable/optional/honest (ADR-150) — готовый чеклист для любых
   форк-интеграций сторонних движков.
