# RuVector: Jev / TypeSafe System One — OpenJev и decision-слой (для pa-jev)

Дата: 2026-10-16. Источник: git-история `/root/vendor-source/ruvector` (MIT, ruvnet/RuVector).
Рабочее дерево стоит на старом HEAD `c6bb23c84` (2026-08-23) — файлов нет; всё ниже читалось через
`git show <sha>:<path>`. Ключевые sha:

- **`950e1cb7d`** — PR #1010: первичный крейт `@ruvector/typesafe` (движок + wire-типы + npm).
- **`425d5ca9d`** — PR #1029 (squash-merge, 2026-09-26/27): **OpenJev** — трейнер `ruvector-typesafe-train`
  (бинарник `openjev`), `ruvector-gpu-runner` (vast.ai), `Engine::train_with_calibration`, bench
  `vs-jev.mjs` + paired e-process, ADR-008, v1-результаты.
- **`df34712cf` / `9f4aad1b3`** — PR #1057/#1060: пост-#1029 доработки typesafe-core (catch-all,
  crossfit-калибровка, SHA-256 receipts, abstain sigmoid, choiceInstructions и др.).

`codebase-memory-mcp` (проект root-vendor-source-ruvector) по запросам openjev/typesafe/systemone
возвращает 0 результатов — индекс построен по старому дереву; годится только для истории до 2026-08.

Существующие отчёты (`ruvector-full-inventory.md`, `ruvector-techniques.md`, `ruvector-crate-map.md`)
тему jev/systemone не покрывают (0 упоминаний) — дублирования нет.

---

## 1. Что лежит (артефакты #1029 и смежные)

Все пути — от коммита `425d5ca9d`, если не указано иное.

### Крейты (Rust, workspace)

| Крейт | Роль | Ключевые файлы |
|---|---|---|
| `ruvector-typesafe-core` | decision-движок: wire-типы, головы, калибровка, банк примеров, receipts, loop-gate | `src/types.rs`, `src/engine.rs`, `src/heads.rs` (+`probe.rs`, `logistic.rs`), `src/calibration.rs`, `src/limits.rs`, `src/loop_gate/sequential.rs` (PairedSequentialTest) |
| `ruvector-typesafe-train` (NEW в #1029) | бинарник **`openjev`**: fetch / prep / prep-check / train / transplant / stage / parity; candle fine-tune bge-small | `src/main.rs` (clap CLI), `src/model.rs` (свой BERT на candle), `src/leakage.rs` (Assertion A), `src/export/transplant.rs`+`pbwalk.rs`, `src/parity.rs`, `configs/openjev-small-v0.toml` |
| `ruvector-gpu-runner` (NEW в #1029) | запуск одного трейн-джоба на арендованном vast.ai GPU: бюджетные шапки, watchdog, GCS-артефакты, audit JSONL | `src/{runner,offer,budget,job,vast,audit}.rs`, README |
| `ruvector-typesafe-ffi` | napi-rs биндинг (`decideJson`/`trainJson`/`optimizeJson`, `train_with_calibration` → опц. поле `calibration` в trainJson) | `src/lib.rs` |
| `ruvector-typesafe-wasm` | тот же контракт в WASM (tract-onnx бэкенд) | `src/lib.rs` |

### Bench (JS, `npm/packages/typesafe/bench/`)

- `vs-jev.mjs` — paired-сравнение локального движка с замороженными ответами реального Jev.
- `lib/paired.mjs` — JS-порт `PairedSequentialTest` (e-process), построчный, сверен golden-векторами
  из Rust (`test/paired-golden.json`).
- `lib/leakage.mjs` — Assertion B (leak-гейт по `train-text-hashes.txt` артефакта).
- `lib/novel-slice.mjs` + `fixtures/novel-slice-2026-09-26.json` — срез «новых композиций» против
  шаблонного заучивания.
- `jev-baseline-2026-09-21.json` — **замороженный захват ответов реального Jev** (test-split, per-item rows,
  оба критерия: baseline и champion); bench играет его офлайн, без сети.
- `gates.json` — релизные гейты; `lib/{gates,metrics,receipt,model-dir,vocab-guard,fixture}.mjs`.
- `results/openjev/openjev-v1-seed2-{test,vsjev}-2026-09-26.{json,txt}` — единственный test-скоринг v1.

### Документы

- `npm/packages/typesafe/docs/adr/ADR-008-openjev-model.md` — **pre-registration** OpenJev (главный документ).
- `docs/research/openjev/v0-plan.md` (план: шаги, команды, гиперпараметры, стоимость) и `v1-results.md`
  (итоги v1 на validation + разбор провала ECE).
- ADR-001…007 в том же каталоге (архитектура, бэкенды, головы/калибровка, self-opt loop, security,
  бенчмарки/гейты, ESP32). ESP32-крейты к jev-слою отношения не имеют.

---

## 2. Wire-контракт Jev в их коде

Эталон — `crates/ruvector-typesafe-core/src/types.rs` (serde) + `npm/packages/typesafe/src/client.ts`
(JS-толерантность) + `src/serve.ts` (сервер). Комментарий в types.rs: имена полей следуют Jev-контракту;
поля `abstain`, `calibrated`, `head`, `model`, `temperature` — **аддитивные** (ADR-003).

### Запрос (`DecisionRequest` / `SystemOneBody`)

```jsonc
{
  "state": "…",              // 1 строка; в serve.ts лимит state 16 KiB (ADR-005), тело ≤ 1 MiB
  "model": "…",              // принимается и ИГНОРИРУЕТСЯ (client.ts)
  "questions": {
    "q1": {"type":"choice","instructions":"…","criteria":{
        "billing":"текст",                              // Criterion::Text
        "fraud":{"what":"…","not_for":"…","examples":["…"]}}},  // Criterion::Structured
    "q2": {"type":"score","instructions":"…","legend":["Calm","Irritated","Angry"]},
    "q3": {"type":"noul","instructions":"the sender needs a response soon"}
  }
}
```

Особенности против ADR pa-jev:
- `questions` — **объект (BTreeMap), не массив**; порядок ключей детерминирован (репродукция probabilities).
- `choice.criteria` — карта `ключ → текст | {what, not_for, examples}`; `not_for` = hard-negative,
  `examples` = дополнительные позитвы прототипа. У ADR pa-jev это соответствует criteria/instructions.
- `score`: у Jev легенда может прийти и как `criteria` (client.ts: `legend ?? criteria`), т.е. pa-jev
  стоит принимать оба поля на входе диалекта `typesafe`.
- `noul`: только `instructions` (предикат одной фразой).

### Ответ (`DecisionResponse`)

```jsonc
{
  "answers": {
    "q1": {"choice":"billing","probabilities":{"billing":0.82,…},   // сумма 1; abstain-масса НЕ здесь
           "confidence":0.78,"abstain":0.05,"calibrated":true,
           "head":"linear-probe","model":"bge-small@sha","temperature":0.47},
    "q2": {"score":1,"legend":"Irritated","probabilities":[0.1,0.7,0.2], …meta},
    "q3": {"noul":0.91, …meta}
  },
  "usage": {"embed_calls":1,"texts_embedded":12,"state_bytes":58}   // ЛОКАЛЬНАЯ семантика, НЕ жетоны Jev
}
```

- 5 аддитивных полей мета: `confidence` (= share·(1−abstain)), `abstain`, `calibrated`, `head`
  (`nearest-prototype|linear-probe|logistic|similarity-uncalibrated`), `model`, `temperature`.
- `serve.ts`: `ServeOptions.jevShapeOnly` **срезает эти 5 полей**, и ответ побайтово совпадает по форме
  с Jev — режим дроп-ин совместимости. Ошибки: `{"error":{"kind":"limit"|"invalid"|"embedder","message"}}`,
  HTTP 400/500; `GET /healthz` → `{status,version,backend}`; bind 127.0.0.1:8787 по умолчанию.

### Как движок это считает (`engine.rs`, `heads.rs`)

- `state` эмбеддится один раз; все тексты вопросов (criteria.what + examples + not_for; legend-бакеты с
  префиксом instructions; noul-предикат) — в **одном батче** `embed`. Компилированные вопросы кэшируются
  по content-hash (`stable_hash`), лимит `MAX_COMPILED_QUESTIONS` (при переполнении кэш сбрасывается).
- Головы: прототипы (zero-shot: mean+L2 по позитвам, минус λ·sim(not_for), λ=0.5), linear probe
  (softmax-классификатор на эмбеддингах train-примеров, полный батч GD), logistic (noul: сбалансированный
  бинарный + Platt). Выбор — по наличию примеров в банке.
- `abstain` считается всегда из **прототипной** геометрии ((K+1)-softmax с abstain-логитом
  `max(best_not_for, (tau−max_sim)/scale)`, tau=0.35, scale=0.5), даже когда share даёт probe.
- `score`-ответ = `round(expected bucket)` по softmax-вероятностям, **не** argmax — ординальная семантика.
- Валидация лимитов — `src/limits.rs` (choice ≤255 опций и т.п., из Jev-контракта).

**Вывод для pa-jev:** types.rs + client.ts — готовый референс полей/допусков для wire-тестов pa-jev
(включая приём `model` и `score.criteria`, BTreeMap-порядок, аддитивную мету с флагом её срезания).
Против ADR pa-jev расхождений контракта не найдено; их тест `jev_shaped_request_round_trips` — образец
doc-фикстуры.

---

## 3. Калибровка и бенч-методология

### Температурная калибровка (`calibration.rs`, `engine.rs`)

- Choice/score: один скаляр T на вопрос, NLL-минимизация, **детерминированный log-grid 120 точек**,
  T∈[0.1, 10]; T не меняет argmax. Noul: Platt `sigmoid(a·s+b)`, 300 итераций GD, lr 0.3, тоже детерминировано.
- Фит — на выделенном **calibration-сплите** (37 tickets), никогда на validation (иначе парный NLL-тест
  gate 2b становится циркулярным — прямо прописано в ADR-008 §4).
- `Engine::train_with_calibration(question, examples, calibration)` (добавлено в #1029): calibration-ряды
  идут в банк с `Split::Calibration`; головы — на всех Train-рядax. Без явного сплита — старый «carve
  каждый 5-й train-ряд», побайтово идентичный (unit-тест). Экспортировано в napi/wasm как опц. поле
  `calibration` в `trainJson`.
- Урок v1 (v1-results.md): фит T на train-карве (градиентные ряды) → модель идеальна на них → T упирается
  в пол 0.1 → на validation confidence 0.9996 при 95.8% accuracy → ECE 0.0537. Правильный hold-out
  calibration-сплит — обязательная часть методологии.
- Второй урок v1: **few-shot режим по умолчанию (--shots 8) сам ломал метрики** (urgent AUROC≈0.54,
  class-prior стёрт); полный data + `probeIterations` 400→5000 (сходимость probe) дал honest-числа.
  Выбор probeIterations — по NLL на calibration-сплите (val-free критерий), default остался 400.
- Пост-#1029: `crossfitCalibration` (opt-in) — кросс-фит температура для банков <20 примеров
  (`ae0eef786`), `abstainMode: sigmoid` — порогируемый OOS-скор (`b2aa6b303`), `choiceInstructions`
  (f20e216d1) — раньше instructions у choice вообще не эмбеддились.

### Paired-сравнение с настоящим Jev (`vs-jev.mjs`, `lib/paired.mjs`)

- Baseline реального Jev **заморожен один раз** в `jev-baseline-2026-09-21.json` (per-item test rows,
  два референса: `baseline` — gen-0 критерии, те же, что шлёт локальная арма; `champion` — мутированные
  критерии, informational, т.к. содержали буквальные train-тексты = меморизация, ADR-004).
  Bench-арм «jev» — **replay без сети**; повторный платный замер — ежеквартально (ADR-006).
- Статистика: anytime-valid paired e-process `PairedSequentialTest` (Rust `loop_gate/sequential.rs`;
  JS-порт с fround-совместимой арифметикой, сверен golden-путями богатства из Rust):
  α=0.05, λ=0.5, порог 1/α=20; на **дискордантных** парах W←W·(1+λ(X−½)); rejекция латчится по
  running max W. Порядок пар пререгистрирован: **лексическая сортировка id** (порядок влияет — фиксируют).
- Тиры на (вопрос × референс): **superior** (forward e-process reject), **non-inferior** (reverse не
  reject И acc ≥ референса), иначе **loses**. Арифметика мощности записана в ADR: rejекция ≈
  `0.223·wins − 0.288·losses ≥ ln 20` (≥14 чистых wins на 150).
- Три среза: full (150) / **novel** (119 — не составленные целиком из предложений train-пула; предложение =
  split `[.!?:]+`, norm, ≥3 токена) / template (31) — против «in-generator only» побед.
- `--strict` — exit 1, если primary-клейм (superior к baseline по всем трём вопросам) не держится.

### Релизные гейты (`gates.json`, ADR-006)

`accuracy_vs_jev` (≥ Jev−3pp), `calibration_ece` ≤0.05 (10 бинов), `urgent/frustration_vs_train_majority`,
`oos_auroc` ≥0.85 (CLINC150), `latency_native_p95` ≤50ms (WASM ≤150ms + parity cosine ≥0.9999),
`transfer_regression` ≤1pp. Гейт без предпосылки = SKIP, никогда тихий PASS. Публикация npm/HF
заблокирована, пока гейты красные.

### Результат v1 (seed 2, test скоринг один раз, `results/openjev/…vsjev…txt`)

- department 93.3% vs Jev baseline 85.3% / champion 90.0% → **non-inferior** (не superior);
- urgent 92.7% vs 61.3%/60.0% и frustration 86.0% vs 67.3% → **superior** (e-process богатство ×337–31356);
- p95 10.1ms, zero leakage; **ECE 0.0543 > 0.05 → гейт провален → веса НЕ опубликованы**
  (HF `ruvnet/openjev-small-v0` не существует). Primary-клейм (superior по всем трём) не met;
  secondary (non-inferior к champion по всем трём) HOLDS. v2 — на свежем held-out наборе.

---

## 4. Методология ADR-008 (pre-registration)

Что именно пререгистрировано (до обучения, в ADR, неизменяемо):

1. **Критерий приёмки = все гейты gates.json на одном gated-ране + test скоринг ровно один раз** для
   финального чекпойнта; validation смещён отбором в пользу OpenJev → любые cross-arm клеймы только по test.
2. **Тиры paired-теста** (§1b): α, λ, порядок пар, пороги — фиксированы; primary/secondary клеймы
   сформулированы заранее, включая «arithmetically out of reach» (superior к champion на 150 невозможен).
3. **Public SOTA-бенчмарки с допуском N=2.0pp** (Banking77 ≥92.06, CLINC150 in-scope ≥95.31) против
   литературы (Mehri & Eric 2020, DialoGLUE), тем же ~33M-энкодером и своими probe-головами;
   HWU64 не клеймится (нет DialoGLUE-сплита в лоадере). Плюс явные ловушки: свежий baseline вместо
   старых 16-shot рецептов (смена протокола = giveaway), novel-composition срез, и т.д.
4. **Leakage-политика (§2)**: градиенты — только train; selection — validation; никогда —
   test/transfer/calibration/Banking77-test/CLINC-test/oos_test/HWU-test. Нормализация `norm`
   (NFKC→lower→не-алфанум руны→пробел→trim) с общими golden-векторами JS/Rust.
   **Assertion A** (тренер, `src/leakage.rs`): до первого шага sha256(norm(text)) всех train/val строк
   против объединённого held-out множества; пересечение >0 → exit 3. Внутренние дубликаты публичных
   сплитов — дропаются из train с подсчётом; коллизии label-описаний (12 имён CLINC150 — буквальные
   test-фразы) исключаются из SupCon-позитивов.
   **Assertion B** (bench, `lib/leakage.mjs`): модель поставляет `train-text-hashes.txt` (сортированные
   sha256 по строкам) — bench пересекает с held-out и **отказывается скорить** при попадании;
   проверка независима от трейнера, воспроизводима любым по артефакту. Отчёты обеих ассерций — дословно
   в model card.
5. **Сиды (§1d)**: клеймится медианный по `best_val_selection` чекпойнт из **5 сидов**, зафиксированный
   ДО test-скоринга; остальные 4 — как variance в model card.
6. **Экспорт = transplant + parity (§5)** (см. §5 ниже) — пререгистрирован как единственный путь
   candle→ONNX, с fallback-планом (CandleEmbedder бэкенд) заранее.
7. **Ночная автономная обкатка (§7)**: `research-nightly-dispatch` оценивает кандидатов (чекпойнты
   по sha256) в offline-контейнере (`--network none`, 4 CPU, kill 85 мин); кандидаты не меняют lockfiles
   (`preflight_scan.py`); перечислены 3 блокера (пин libonnxruntime в образе, гидрация 133MB FP32 графа
   и датасетов до контейнера, бюджет CPU на 5×suite).

**Тренировка** (`configs/openjev-small-v0.toml`, `model.rs`, `train/`): bge-small-en-v1.5 (33M, 384d),
SupCon-лосс (темп. 0.05, P×K сбалансированные батчи, критерии класса как экстра-позитивы) + мультитаск
головы (отбрасываются при публикации — движок фитит свои), мягкие таргеты 0.75/0.25 для ambiguous,
outlier exposure на CLINC oos_train, label smoothing 0.1, 2×AdamW, warmup 6% + linear decay, clip 1.0,
max_seq_len 64, 3000 шагов. Своё BERT-ядро на candle (в candle-nn 0.9.2 fused LayerNorm/softmax без
backward — молча рвёт градиент). Стоимость v1: **$0, локальная RTX 5080, ~5 мин/сид**; vast.ai не
понадобился (`ruvector-gpu-runner` остался как инфраструктура: обязательные `--max-usd/--max-hours`,
watchdog на 95%, destroy-on-anything, digest-пины образа, git-sha assert, GCS-артефакты, audit JSONL).

### ONNX weight-transplant + ort-parity (§5) — механика

- candle не умеет писать ONNX → веса **пересаживаются в уже запиненный базовый граф**
  (`models/bge-small-en-v1.5/model.onnx`, sha256 `828e1496…`, грузится и в ort, и в tract 0.23):
  1. декодировать ModelProto через `tract_onnx::pb` (prost);
  2. каждый float-инициализатор (всего 197: 125 именованных + 72 анонимных `onnx::MatMul_*`, хранятся
     транспонированно) сопоставить **ровно одному** тензору базовых safetensors по значению (прямое или
     транспонированное совпадение, tol 1e-6); кросс-чек по структуре: именованный — по имени, анонимный —
     по имени потребляющего узла (`/encoder/layer.0/attention/self/query/…`); неоднозначность/расхождение → abort;
  3. перезаписать payload-байты инициализаторов на месте (`pbwalk.rs::float_payload_spans`), топология не
     трогается; потребовать биекцию на все тензоры энкодера;
  4. **Parity = доказательство корректности**: 512 probe-текстов, ort(ONNX) vs candle(safetensors):
     cosine ≥0.9999 на каждый текст + идентичные department-решения движка, построенного на каждом.
     Факт v1: cosine min ≥ 0.99999999999, 256/256 решений идентичны. `--identity` (базовые веса) —
     round-trip smoke-тест нулевого шага.
- INT8 (stretch): те же transplant в `model_quantized.onnx` с перекватизацией весов per-tensor в Rust.
- Следствие: смена архитектуры (bge-base, другой пулинг) = новый запиненный шаблон, не только веса.

---

## 5. Переносимость в pa-jev

### (a) Их Jev wire-типы как референс — ДА, переносится напрямую

- `types.rs` (serde) + `client.ts` (`SystemOneBody`, толерантности: `model` игнорируется, score принимает
  `criteria|legend`) + `serve.ts` (`/v1/systemone`, `/healthz`, `jevShapeOnly`-срез аддитивной меты,
  error-shape) — готовый чек-лист для wire-диалекта `typesafe` в pa-jev и его doc-фикстур
  (у них фикстура `jev_shaped_request_round_trips` прямо в types.rs).
- Расхождения контракта с ADR pa-jev не найдены; наоборот, найдены 3 точки толерантности, которые
  стоит заложить в pa-jev-клиент: объект questions (не массив), score `criteria` как алиас `legend`,
  поле `model` в теле. Их `usage` — локальная семантика (`embed_calls/texts_embedded/state_bytes`),
  не жетоны: pa-jev-клиент должен маппить на `Usage { cost }` по каталогу, как в ADR.

### (b) Калибровка/бенч-методология для валидации pa-jev против реального API — ДА, ядро переносимо

Рецепт, который стоит повторить у нас (почти без кода):
1. **Один замороженный захват реального API** (per-item rows, критерии + answers + usage) → офлайн-replay
   в CI без сети и без повторных трат; периодический платный re-capture с датой.
2. **Paired anytime-valid e-process** (α=0.05, λ=0.5, порог 20, дискордантные пары, лексический порядок
   пар пререгистрирован) + тиры superior/non-inferior/loses в обе стороны — вместо point-estimate
   «наши vs Jev». JS-порт ~100 строк, сверяем golden-векторами.
3. **Калибровка как измеряемый гейт** (ECE 10 бинов ≤0.05, Brier, reliability bins в receipt) +
   обязательный hold-out calibration-сплит для T/Platt (их провал ECE 0.0543 именно на этом слое);
   учёт jagged edges: числа только под запинованную версию модели.
4. **Leakage-ассерции по хешам нормализованного текста** (норм с golden-векторами в двух языках,
   train-text-hashes.txt в артефакте) — если pa-jev будет вообще где-то фитить пороги/температуры
   на своих данных (answer-cache статистика, shadow-режим).
5. **Методологические ловушки, задокументированные кровью**: (i) few-shot сэмплинг стирал class-prior и
   убивал AUROC — полный data обязателен; (ii) probe не сходился при 400 итерациях; (iii) validation
   смещён любым отбором — клеймы только по тронутому один раз test; (iv) шаблонные данные → срез
   novel-composition; (v) гейт без предпосылки = SKIP, не PASS.
6. `--strict`-флаг бенча и receipt-дисциплина (каждый ран = JSON-receipt с хешами фикстур) — образец
   «verifiers over self-assessment», совпадает с правилами нашего репо.

### (c) Self-hosted openjev как дешёвый локальный кандидат Judge — ЧАСТИЧНО, с оговорками

Что уже есть у них: `typesafe serve` — дроп-ин HTTP-сервер Jev-контракта (127.0.0.1:8787, healthz,
jevShapeOnly) поверх локального движка; нативный p95 ~10ms FP32 / ~4ms INT8 (bge-small, 33M, 384d),
$0, офлайн; engine детерминирован. Для pa-jev это выглядело бы как ещё один диалект/кандидат
в цепочке вида `local/openjev` (native-only правило соблюдается: вероятности честные, калиброванные).

Оговорки:
- **Весов нет**: HF `ruvnet/openjev-small-v0` не опубликован (ECE-гейт провален). Свой вариант придётся
  тренировать самим: весь пайплайн MIT и воспроизводим (тренер ~5 мин/сид на потребительской RTX 5080,
  $0; transplant+parity; cpu-runner для vast.ai, если GPU нет). Это проектная работа, не «скачать модель».
- **Качество на off-topic**: их же замер — explicit `other` ловит 17% off-topic при 24% ложных
  (Jev: 77%/1%); abstain без `sigmoid`-режима не порогируется между вопросами; у choice instructions
  исторически не читались (добавили opt-in постфактум). То есть как замена Jev на OOS/abstain-границах —
  слабо; как классификатор по явному меню опций в домене с train-примерами — работает (93% на 8 классов).
- Контракт обучения: criteria приходят в запросе, головы фитятся на лету из примеров → семантика ближе
  к «классификатор с меню», чем к судье общего назначения; для наших потребителей (auto-thinking tier,
  tool arbiter, re-rank) — подходит, если у каждого вопроса своё меню и examples.
- Если pa-jev возьмёт self-hosted вариант, то интеграция — по HTTP `POST /v1/systemone` (как внешний
  диалект) или прямой порт ядра движка (heads+calibration ~1.5k строк Rust) в pa-jev без npm/napi.

### (d) Что НЕ переносится

- **npm/napi/wasm-упаковка** (`@ruvector/typesafe`, ffi, wasm, платформенные пакеты) — у нас свой
  Rust-нативный pa-jev, биндинги не нужны.
- **ruvector-gpu-runner** (vast.ai) — у форка нет такого пайплайна; сами по себе паттерны (обязательные
  бюджетные шапки, watchdog на 95%, destroy-on-anything, digest-ины, audit) — хорошая выжимка, если
  когда-нибудь арендовать GPU.
- **Их движок как зависимость**: тянуть крейты ruvector в workspace форка = новая внешняя зависимость
  (по AGENTS.md — архитектурное решение); каламбур с pa-types-владением решается копией паттернов,
  не импортом.
- **Датасеты/фикстуры** (синтетические тикеты 8 департаментов, CLINC150/Banking77/HWU64 лоадеры,
  публичные SOTA-сравнения) — англоязычные intent-домены; для валидации pa-jev нужны свои золотые
  наборы (но каркас harness переносим).
- **Публикация весов/HF-путь, research-nightly-dispatch** — заблокирован и завязан на их CI-инфраструктуру.
- **Transplant-механика** нужна только если форк сам fine-tune'ит энкодер под свой ONNX-рантайм;
  при использовании чужих готовых весов она избыточна (но это красивый образец «proof by parity»,
  применимый к любому своему экспорту в ONNX — у нас ort уже в зависимостях pa-**?** — фактически
  переносима идея: value-match + structure cross-check + in-place rewrite + cosine-parity proof).

## 6. Выводы

1. RuVector — единственный найденный открытый код, где **контракт Jev реализован дважды с обеих сторон**
   (клиент-совместимый движок `decide/systemOne` и сервер-дроп-ин `POST /v1/systemone` с jevShapeOnly) —
   это лучший доступный референс для wire-диалекта `typesafe` в pa-jev; расхождений с нашим ADR нет,
   есть 3 полезные толерантности.
2. Готовых «openjev-весов» **не существует** (v1-гейт ECE 0.0543 провален, публикация запрещена
   pre-registration'ом) — self-hosted кандидат требует собственной тренировки; вся методика для этого
   открыта и дешева (~$0, ~5 мин/сид), но качество на off-topic заведомо хуже Jev (17%/24% vs 77%/1%).
3. Самая переносимая ценность — **бенч-методология**: замороженный захват реального API + офлайн-replay,
   paired anytime-valid e-process с пререгистрированными тирами/порядком, ECE-гейт, hold-out
   calibration-сплит, leakage-ассерции по хешам, test-скоринг один раз. Это ровно то, чего требует
   ADR pa-jev («валидация ответов pa-jev против реального API») и что можно реализовать в pa-jev
   малым кодом (~300-400 строк Rust/JS).
4. Для pa-jev рекомендация: взять (a) types.rs/client.ts/serve.ts как wire-референс и источник
   doc-фикстур; (b) paired.mjs/gates.json/novel-slice как скелет валидационного harness; отложить (c)
   self-hosted openjev до Phase 2+ и только если появится своя тренировочная задача с явным меню опций;
   не тянуть (d) их крейты/npm/gpu-runner как зависимости.
