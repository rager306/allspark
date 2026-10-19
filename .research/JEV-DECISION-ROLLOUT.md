# JEV decision rollout: метод, локальные пробелы, acceptance

Дата: 2026-10-19. Статус: **план исследования, не runtime cascade и не доказанная экономия**.
Код не изменялся; тесты/бенчмарки для этого документа не запускались.

## Источник и границы доказательств

- Полная доступная заметка: `/root/vault/inbox/jevagentharness-decision-models-share-control-with-llms.md` (уже сохранена пользователем; указан commit `ed49fb9`, заново vault не меняем).
- Стабильная копия: [sources/jevagentharness-vault.md](sources/jevagentharness-vault.md).
- SHA-256 обеих копий: `7134815ebfb8e5d6a64fcf975716e1e6965d6ecf45c756766d4028ec44b0456b`. Совпадение проверено локально.
- Заметка ссылается на arXiv `2610.21412v1`, полный текст которого, по её provenance, прочитан 2026-10-19. Здесь использована именно эта полная заметка, **не новая веб-проверка статьи или репликация**.
- Предыдущие сводки: [learn_arsumbris_jev_harness.md](learn_arsumbris_jev_harness.md), [SYNTHESIS-2026-10-19.md](SYNTHESIS-2026-10-19.md). Их «−14% без потерь» — результат чужого эксперимента, не прогноз для allspark.

## Что утверждает источник (не локально воспроизведено)

Восемь decision points: tool, arguments, action, write memory, reflect, stop,
route, read back. Пять режимов: Replace, Assist, Guard, Cascade, Trigger.
Рабочая последовательность: **shadow → threshold per decision point fit → live
noninferior + cheaper → freeze**. Изолированная точность не доказывает качество
живого агента: L1/L2 и L3 измеряют разные вещи.

| Результат по заметке | Область применимости |
|---|---|
| Jev + Flash: fitted thresholds, −14.2% стоимости, −0.0 п.п. успеха; 150 calibration tasks; tools τ=1.00, args τ=0.99 | Конкретные агент, LLM, задачи и распределение источника; не наши defaults |
| Без fitted threshold −19.0 п.п.; threshold 0.5: −21.5 п.п. | Негатив против произвольного глобального порога |
| Flash-Lite: все решения остались LLM, экономия 0 | Валидный исход fit; не надо ослаблять критерий ради adoption |
| AgentDecision-4B: −86.3% стоимости, 2.5× быстрее, указанная граница потери 3 п.п. | Отдельная обученная модель/DecisionGym; **не coding benchmark** |
| Coding: routing strong/fast LLM даёт только 7.0–10.5%; надёжная общая экономия не установлена | Не переносить 86.3%, 14.2% или охват tool/args на coding |
| stop guard принял 40.7% неверных остановок; все 34 провала Jev начались с необратимого решения | Guard не security boundary; необратимые действия никогда не делегировать |
| Смена правил без refit: 3.0% против 88.0% LLM; refit восстановил результат | Drift инвалидирует прежние thresholds |

Формула: `c(s) >= q_L - ΔC/V`. Здесь `c(s)` — эмпирически откалиброванная
уверенность decision model, `q_L` — качество базового LLM **на данном decision
point**, `ΔC` — ожидаемая экономия вызова, `V` — стоимость ошибочного выбора
в совместимых единицах. Принятое в статье `$1/задача` не является нашей оценкой
ущерба. Для необратимых действий формула не даёт разрешение: они исключены
независимо от confidence.

Thresholds специфичны к **agent + LLM + task distribution + decision point**,
а также версии decision model, prompt/правил, roster и схемы опций. Нельзя
использовать один 0.8 или переносить τ=1.00/0.99 без локальной калибровки.
Сырой provider confidence и agreement с LLM ещё не есть вероятность правильности.

## Что проверено локально чтением текущего дерева

Структурный поиск: codebase-memory `root-prime-agent`, index ready, но snapshot
2026-10-18. Положительные выводы ниже сверены с текущими файлами, поэтому номера
строк могут отличаться от index. Это статический аудит, не runtime свидетельство.

| Проверенный факт | Код/граница |
|---|---|
| `pa-jev` — leaf transport/wire types, не session/policy owner | `crates/pa-jev/README.md`, `src/lib.rs:1–28` |
| Settings, candidate chain, cache и consumers живут выше | `crates/pa-core/src/jev/mod.rs`, `resolve_judge:153`; pinned default model на строке 55 |
| Tool arbiter — deterministic suggestion + необязательный judge; только advisory, исходная bash-команда не подменяется | `crates/pa-core/src/jev/tool_arbiter.rs:1–42`, `note:696` |
| Judge отвечает на `use_specialized / use_bash / unsure`; state содержит обрезанную команду и совет, не полный decision state | `arbiter_request:478`, `judge_choice:637`; это не выбор всего tool roster и не args generation |
| `STRONG_CONFIDENCE=0.8` меняет только тон hint; не admission threshold cascade | `tool_arbiter.rs:59`, `ArbiterVerdict::is_strong` |
| Ошибка/timeout сохраняет deterministic hint; после 2 failures judge suspends, command остаётся неизменной | `note:696`, `MAX_CONSECUTIVE_FAILURES`; fail-open advisory не означает разрешение опасной операции |
| Gates проверяют settings и native chain, не calibration artifact | `should_run_tool_arbiter:736`, `arbiter_admitted:757` |
| Offline eval уже имеет frozen corpus, dev/test split, fingerprint, scripted judge, threshold tune на dev и gate на test | `crates/pa-core/src/jev/eval/mod.rs`; scripted ответы явно не замена live run |
| Eval считает accuracy/F1/confusion/fallback/p95; его tune выбирает required accuracy delta, а не `c(s)` per decision point | `SplitReport:401`, `TuningPolicy:558`, `TunedThresholds::tune:620`, `gate:739` |

Заявление vault «shadow уже есть как бинарный арбитр сессий» нельзя считать
подтверждением полного shadow pipeline именно tool arbiter. В просмотренных
`pa-core/src/jev` и `pa-jev/src` нет реализации end-to-end shadow trajectory
collector/fitter/noninferiority deployment record. Это ограниченный scope-аудит,
не утверждение, что вне этих каталогов нет исследовательских прототипов.

## Конкретные пробелы и владение

1. **Shadow observation contract** (`pa-core`, session seam): фиксировать один
   decision point, candidate set, baseline LLM choice, judge probabilities,
   eligibility, measured cost/latency, независимый outcome и версии. Нельзя
   менять выполненное действие в shadow. В репозитории только обезличенные
   fixtures: не raw prompts, user files, auth или session identifiers.
2. **Calibration fit** (offline исследовательский путь, не `pa-jev` transport):
   truth labels + calibration curve/Brier/ECE и uncertainty по каждому point;
   считать `q_L`, `ΔC`, `V`, coverage и ошибки на post-error/shifted states.
   Agreement не подменяет truth. Threshold artifact связывать с corpus hash и
   полным tuple версий; никакого automatic переносa provider-fallback threshold.
3. **Economic admission**: существующий eval accuracy-win gate не доказывает
   noninferiority конечной задачи и net savings. Нужны end-task paired outcomes,
   judge+fallback+retry costs, cache-aware LLM billing и latency, не только
   дешёвая цена judgment request. Не ослаблять существующий gate автоматически.
4. **Eligibility/security boundary**: irreversible actions исключены до model
   call; deterministic validation, permissions и human approval остаются
   независимо от Replace/Assist/Guard/Cascade/Trigger. Coding shell/free-text
   нельзя объявлять безопасным только по названию «read».
5. **Promotion/freeze**: нет проверенного deploy artifact с fitted threshold,
   holdout/live evidence, выбранной единственной делегацией и rollback policy.
   Advice `STRONG_CONFIDENCE` не превращать в cascade patch.
6. **Drift**: version mismatch, новая rule/task/model/roster, coverage/ошибки или
   calibration drift возвращают конкретную делегацию в shadow/LLM baseline.
   Смена vendor цепочкой fallback не доказывает одинаковую калибровку.

Новых crates/dependencies/public APIs этот документ не вводит. Если появится
реализация, `pa-jev` остаётся leaf; consumer policy и session wiring не должны
переехать в transport. Любые интерфейсные изменения требуют отдельного review.

## Acceptance plan — по одной делегации, без обещания выигрыша

### 0. Зафиксировать baseline и критерии до измерений

Начальный кандидат для coding — **routing** только между доступными LLM,
не автоматическое выполнение tool args. Это гипотеза из статьи, не рекомендация
включать routing сейчас. Отдельно определить reversible allowlist и запрет
irreversible effects. Записать task families, model/prompt/roster versions,
правила, стоимость и ценность ошибки, seed/task split и бюджет live опыта.
Размер выборки определить power/uncertainty анализом, не копировать 150/500 из
статьи. Noninferiority margin и cost minimum согласовать заранее; **не задавать
новый числовой production default этим документом**.

### 1. Shadow

Запустить исходного агента без изменения решений. Собрать решения/оценки на его
собственных траекториях, включая ошибки и shifted rules. Независимый verifier
читает state/files/exit codes, не prose. Проверить, что shadow не меняет tool
calls, исходы и permission checks. Нет credentials, trace-local identifiers или
user content в публичных архивах. Разделить fit/dev и untouched holdout по задачам,
не по соседним шагам одной траектории.

### 2. Threshold per decision point fit

На fit/dev оценить calibration и формулу; freeze thresholds и версии до holdout.
Сравнить детерминированный baseline, LLM baseline, fixed-threshold negative
control и fitted policy. Результат «ни одно решение не прошло» принимается.
Слабые/пустые slices, неизвестная калибровка, invalid confidence, timeout,
неразрешённый action или version mismatch → baseline LLM, не более низкий τ.

### 3. Live: noninferior **и** cheaper

Только reversible decision point и только явно разрешённый эксперимент. На
holdout и live paired задачах проверить end-task success; заранее выбранная
нижняя confidence bound разности должна быть не ниже `−margin`. Одновременно
верхняя confidence bound разности полной стоимости должна быть ниже нуля
(либо строже заранее согласованного minimum savings). Репортировать абсолютные
числа, CI, coverage, fallback/retry costs, tail latency и slices отдельно.
Offline L1 accuracy, scripted judge или несигнификантный «0 потерь» не заменяют
этот gate. Coding savings могут оказаться 0 или отрицательными.

### 4. Select → freeze → monitor

Продвинуть только доказанную делегацию; остальные оставить baseline/shadow.
Заморозить task/model/prompt/roster/schema identities, corpus hashes, calibration,
thresholds и evidence report. Изменения требуют нового fit/validation. Drift
или regression → rollback к LLM; irreversible operations всегда вне delegation.

### Будущие тесты (не заявлены выполненными)

- Unit: admission/version/eligibility boundaries, calibrated lookup и fallback.
- Provider wire: именно request/probability decoding при изменении wire.
- Integration: shadow не влияет на action; timeout/error сохраняет baseline.
- Real-model QA: semantic routing/tool choice и end-task invariants; faux не
  доказывает правильный выбор модели.
- Каждый bug fix — свой минимальный regression red-first в tier, где жил баг;
  для будущего cascade нужны отдельно воспроизводимые live evidence и контроль
  leakage. Положительный benchmark заранее не обещается.

## Решение сейчас

**Runtime не менять.** Сначала provenance-safe shadow protocol и заранее
согласованные acceptance criteria. Существующий advisory arbiter сохранить
advisory; Guard никогда не объявлять security boundary. В синтезах читать
«−14% без потерь» как claim источника, а не локальный факт или roadmap SLA.
