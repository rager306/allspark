# Command Code: tool-calling + Taste — углубление и проверка на живом коде

Дополняет `.research/commandcode-toolcall-repair.md` (далее — «наш отчёт»). Всё, что ниже,
либо **проверено исполнением** вырезанных из бандла функций (node, v1.84.0), либо имеет
точное смещение в `dist/cli.mjs`, либо — баг-репорт GitHub с номером.

Бандл: `/usr/lib/node_modules/command-code/dist/cli.mjs`, 3 362 140 байт, версия 1.84.0.
Скрипты проверки: `/tmp/cc_verify/*.cjs` (repair.cjs, coerce.cjs, reorg.cjs, run.cjs, cmp.cjs, lint.cjs).

---

## 0. Что НОВОГО относительно нашего отчёта (краткая сводка дельты)

| # | Наш отчёт | Факт (проверено) |
|---|---|---|
| 1 | «13 именованных правил» | **12** в каталоге `zf` (@1932672). Тринадцатого нет. |
| 2 | правило 7 `wrapBareScalarAsArray` — «скаляр → [скаляр]» | Срабатывает **только** когда тип поля `array`; для строкового поля массив **не** чинится ничем (проверено). |
| 3 | `stripOptionalMarkerFromKey` — «`pattern?` → `pattern`» | Срабатывает для **`?` и `*`** (Vf `["?","*"]`), и только если срезанный ключ «осмыслен» (`isWorthStripping`). |
| 4 | coerceToolInput «голая строка → wrapBareStringRoot» | Плюс: **числовой/булев корень (42, true) молча превращается в `{}`** — не восстанавливается. |
| 5 | alias-таблица `Rf` | **22 тула × N полей** (≈200 алиасов; в отчёте перечислено ~40). Есть баги (см. §5). |
| 6 | reject «не stack trace» | Есть **два разных** reject-а: schema-reject (`Invalid input for tool "X"`) и permission-reject (`No tool named "X" exists`) — разные пути. |
| 7 | — | **`requiredAlternatives`** и **`exactlyOneOf`** — второй контур валидации, в отчёте не упомянут. |
| 8 | — | `repairToolInput` **падает TypeError** на схеме без `properties`/`required` (не защищён). |
| 9 | — | `reorganizeIfNeeded` **теряет правила** при slug-коллизии и падает `ENAMETOOLONG`; баг открыт с 1.11.1 по 1.73.0. |
| 10 | — | `compareLearnings` **не имеет сигнала удаления** правила (deleted=тишина). |
| 11 | — | Два разных парсера правила: `extractLearningWithConfidence` (runtime) vs `parseLearning` (sync/lint) — **несовместимы**. |
| 12 | — | Промпт-секция `<taste_guidance>` (@2049577) — 3.4 КБ «как пользоваться taste», в отчёте её нет. |
| 13 | — | Правило разрешения конфликта «taste vs запрос» **есть**, в промпте (@2052400). |
| 14 | — | `taste lint` — реальный валидатор формата (@1416311…), 8 диагностик + 12 suggestions. |

---

## 1. Схема описания тула: поля, длина, примеры, «when NOT to use»

**ID:** `tool-schema-shape`
**ИСТОЧНИК:** `cli.mjs` — таблица тулов @325058…@1893373; пример эталона `todo_write` @1893373.
**СИМПТОМ / ПРИЁМ**

Схема тула = `{ name, label, description, input_schema }` (+ рантайм-флаги `readOnly`,
`isConcurrencySafe`, `shouldDefer`, `searchHint`, `visible()`, `maxResultSizeChars`,
`requiredAlternatives`, `exactlyOneOf`). Поля description в `input_schema.properties`
имеют **свои** описания — они и есть главный обучающий канал (модель читает их чаще, чем
описание тула).

Замеренные длины `description` (символы; всего 30 тулов имеют статическое описание, остальные — динамические: `agent`, `web_search`, `sleep`, `codemode`, `activate_skill`):

| тул | len | есть «When NOT» | есть пример |
|---|---|---|---|
| `read_file` | 2899 | — | — |
| `shell_command` | 2625 | — | — |
| `todo_write` | 1918 | **да** | **да** |
| `enter_plan_mode` | 1536 | — | — |
| `glob` | 1388 | — | — |
| `edit_file` | 1351 | — | — |
| `grep` | 954 | — | — |
| `taste` | **139** | — | — |

Единственный тул с полной «дисциплиной описания» — `todo_write`:
`## When to use it` → `## When NOT to use it` → `## Rules` (5 пронумерованных) →
`## Each item` → `## Example call` (JSON дословно). Это **эталон**, а не общая практика:
у 29 из 30 тулов со статическим описанием нет ни «when NOT», ни примера.

Отдельно: у `grep` и `glob` «when NOT» живёт **в тексте**, инлайном:
`grep`: «ALWAYS use this grep tool for search tasks. NEVER invoke `grep` or `rg` as a shell
command.» (@1799820); `powershell`: «Use the dedicated tools for ordinary file work: glob for
file search (not recursive Get-ChildItem), grep for content search (not Select-String),
read_file (not Get-Content), edit_file / write_file (not Set-Content or Out-File)»
(@418412). Это ровно тот приём «grep→find», который нас интересовал, — и он **не в taste**,
а в описании тула + в базовом промпте (см. §3).

**НАШ АНАЛОГ:** трейт уже гибкий — `fn description(&self) -> &str`
(`crates/pa-agent/src/types.rs:538`), не `&'static str`; ограничение только в
**реализациях**, которые возвращают строковые литералы. Их вариант — геттер
`get schema(){…}`, пересобирающий описание на каждый вызов (@841181), — нам не нужен:
достаточно хранить `String` в структуре тула. Наши описания:

| тул | файл:строка | длина |
|---|---|---|
| `graph` | `crates/pa-core/src/tools/graph.rs:573` | **518** симв. |
| `lsp` | `crates/pa-lsp/src/tool.rs:958` | **503** симв. |
| `edit` | `crates/pa-core/src/tools/edit_hybrid.rs:298` | **174** симв. |
| `jev` | `crates/pa-core/src/jev/tool.rs:371` | **256** симв. |

То есть у нас **в 2 раза короче** их медианы (~950) и в 5.6 раза короче их максимума
(2899); `edit` — короче их `taste` (139) лишь на 35 символов. При этом `edit` прямо признаётся в комментарии: «A short, static headline; the mode
list is appended at runtime via `label`/schema. The static form keeps the `&'static str`
contract» (@296) — то есть длина ограничена **привычкой к литералу**, а не трейтом. Секций «When NOT», «Rules», «Example call» нет ни у одного из 4. Секций «When NOT», «Rules»,
«Example call» нет ни у одного из 4 нативных тулов; `parameters()` — сырой JSON Schema
без per-field описаний-инструкций (в `graph.rs:681+` описания полей есть, но это
перечисление действий, не «когда НЕ надо»).

**ЧТО ДЕЛАТЬ: перенять.**
1. **Снять ограничение в реализациях, а не в трейте**: трейт уже отдаёт `&str`
   (`types.rs:538`), поэтому достаточно положить `description: String` в структуру
   каждого тула и вернуть `&self.description`. Правка = 4 структуры + 4 геттера;
   `system_prompt.rs` уже работает со строками (`pa_edit::description(mode)`).
2. Ввести конвенцию `## When to use` / `## When NOT to use` / `## Example call` в
   `description()` для `lsp`, `graph`, `edit`, `jev` — начиная с самой конфликтной пары
   (`graph.search` vs `lsp.find_symbol` vs `bash grep`).
3. **Регрессионный тест** (вход → ожидание): для каждого нативного тула
   `description()` содержит подстроку `"When NOT"` и блок `"Example"`;
   `description().len() >= 800`; каждое `required`-поле имеет непустое `description`
   в `parameters()["properties"][field]["description"]`. Файл-образец:
   `crates/pa-core/tests/prompt_guards.rs` (там уже есть гард на упоминание имён тулов).
4. Проверять «when NOT» на **парность**: у каждого правила «используй X» должен быть
   назван антипод («не делай Y») — иначе модель не понимает границу.

**СЕРЬЁЗНОСТЬ: высокая** (дешевле любой другой меры; 0 токенов латентности).

---

## 2. Механика taste: запись, confidence, конфликты, откат

### 2.1 Формат и парсеры — их ДВА, и они несовместимы

**ID:** `taste-two-parsers`
**ИСТОЧНИК:** `extractLearningWithConfidence` @781801 (runtime, сравнение версий);
`parseLearning` @1402315 (sync/lint/импорт).
**СИМПТОМ** (проверено, `/tmp/cc_verify/c8.cjs`, `c9.cjs`):

```
"- Prefers tabs. Confidence: 0.9"   runtime: {text:"Prefers tabs", 0.9}   lint: ok
"- Prefers tabs Confidence: 0.9"    runtime: null  (!)                   lint: {text:"Prefers tabs"}
"- rule. confidence: 0.9"           runtime: null  (!)                   lint: ok
"- Rule. Confidence: 1.5"           runtime: {confidence:1.5} (!!)       lint: ERROR out of range
```

Runtime-регексп `^\s*-\s+(.+?)\.\s+Confidence:\s*(\d*\.?\d+)` **требует точку** перед
`Confidence:` и **точный регистр**. Lint-регексп `^-?\s*(.+?)[\.:,]?\s*confidence:` —
регистронезависим и допускает `,`/`:`. Следствие: правило, которое `taste lint` считает
валидным, **не даёт notice и не попадает в diff** при записи (молчаливая слепота
телеметрии), а правило с `Confidence: 1.5` runtime принимает как «апгрейд до 150 %».

**НАШ АНАЛОГ:** `crates/pa-core/src/refinement/*` — структурированный JSON
(`HarnessEntry{id,kind,title,content,...}`), парсера строк нет вовсе, поэтому класса
багов «два парсера» у нас не существует. Это наше преимущество — **не терять его**.

**ЧТО ДЕЛАТЬ: не перенимать markdown-формат.** Если понадобится экспорт в markdown
(для чтения человеком/шаринга) — писать **из** структуры, никогда не парсить обратно
единственным источником истины. Регрессионный тест: round-trip `HarnessState → md → md`
идемпотентен; и отдельно — «md, отредактированный человеком, не подменяет state без
явного импорта».

**СЕРЬЁЗНОСТЬ: средняя** (у нас нет бага, но есть риск при копировании формата).

### 2.2 Confidence: откуда берётся и чего не делает

**ID:** `taste-confidence-is-model-fiction`
**ИСТОЧНИК:** sysprompt learner'а @1996187; `compareLearnings` @782208; `emitLearningSignals` @785877.
**СИМПТОМ**

- Значение confidence **целиком выдумывает learner-модель**. В коде нет ни формулы, ни
  счётчика подкреплений, ни затухания. Единственное правило — строка в промпте:
  «do NOT raise or lower an existing learning's confidence unless the NEW messages
  themselves contain fresh evidence for it».
- `compareLearnings` сравнивает **точное совпадение текста правила**. Переформулировка
  = «added», не «upgraded» (проверено: `Use pnpm` → `Use pnpm for installs` даёт
  `added:[{Use pnpm for installs}]`, а старое правило остаётся).
- **Удаление правила не даёт сигнала вообще**: `compareLearnings(old:"- X", new:"")`
  → `{added:[],upgraded:[],downgraded:[]}`. То есть «taste downgraded» в UI означает
  только «модель написала меньшую цифру», а исчезновение правила невидимо.
- Валидатор `taste lint` (@1416311) ругается на `Confidence: 0.0` (warning «may not be
  useful»), но **ничего не делает** с 1.5 в runtime-пути.

**НАШ АНАЛОГ:** в `HarnessEntry` поля confidence нет; в `RefinementResult` есть
`evidence`/`outcome`, но нет числовой уверенности и нет дедупликации по смыслу
(`crates/pa-core/src/refinement/planner.rs:193 validate_edit` проверяет только
id/slug/kind).

**ЧТО ДЕЛАТЬ: перенять идею, но с кодом вместо промпта.**
- Ввести `confidence: u8` (0–100) в `HarnessEntry` **и** пересчитывать его **кодом** из
  наблюдаемых событий: `+X` за независимое подтверждение, `-Y` за успешный rollback,
  `-Z` за «правило не применилось N сессий». Никогда не спрашивать у модели число.
- Ввести `last_confirmed_at` + decay-политику (см. §2.4).
- **Регрессионный тест:** `apply(evidence_kind=Contradicted) → confidence` строго
  меньше; `apply(evidence_kind=Confirmed)` → строго больше; `apply(evidence_kind=NoOp)`
  → без изменений; удаление правила порождает `RefinementAction::Delete`-событие
  (у них — тишина).

**СЕРЬЁЗНОСТЬ: высокая** (это ядро «taste» как механизма; без него taste = файл,
который кто-то правит вручную).

### 2.3 Разрешение ПРОТИВОРЕЧИЙ между правилами

**ID:** `taste-conflict-resolution`
**ИСТОЧНИК:** `<taste_guidance>` @2049577 (дословно @2052400); sysprompt learner'а @1996187.
**СИМПТОМ / ПРИЁМ**

Механизма разрешения конфликтов **в коде нет**. Есть три мягких договора:

1. В промпте главного агента, дословно:
   «**If a taste preference conflicts with the user's current request, follow the current
   request (it may be updating the preference)**» (@2052400).
2. Там же: «Taste preferences override general best practices — if taste says "Use X", you
   use X even if you would normally prefer Y».
3. В промпте learner'а: «Prefer amending existing files over creating near-duplicates» —
   то есть дедупликация отдана модели, не коду.

Порядок приоритетов, который из этого следует (и который стоит перенять как **явную**
лестницу, а не как текст): `текущий запрос пользователя` > `taste` > `общие best practices`.
Никакой проверки на противоречие между двумя строками taste нет; `taste lint` тоже не
ищет противоречий (проверено: только формат/диапазон/пустые категории).

**НАШ АНАЛОГ:** `format_harness_state_for_prompt` (`crates/pa-core/src/refinement/ranking.rs:195`)
выдаёт промпт-ноту «Keep refine edits small and evidence-backed», но лестницы приоритетов
и правила «запрос пользователя важнее memory» в промпте нет.

**ЧТО ДЕЛАТЬ: перенять.**
1. Вписать в `format_harness_state_for_prompt` три строки приоритета (запрос > entry >
   общие практики) — дёшево и снимает самый частый вред от memory.
2. Добавить **кодовый** conflict-check: при `RefinementAction::Create` считать
   пересечение термов (у нас уже есть `harness_query_terms` + IDF в `ranking.rs:43,108`)
   с существующими entry того же kind; при пересечении ≥ порога — требовать
   `action: Update` с явным `id`, иначе отклонять. Это ровно то, что у них делает промпт
   («prefer amending»), только детерминированно.
3. **Регрессионный тест:** `create` с текстом, чьи термы на 80 % совпадают с существующим
   memory → `Err(NeedsUpdate{existing_id})`; `create` с новыми термами → `Ok`.

**СЕРЬЁЗНОСТЬ: высокая.**

### 2.4 Устаревание / откат плохого правила

**ID:** `taste-no-staleness-no-rollback`
**ИСТОЧНИК:** issue #382 (open, «Taste fails to update obsolete info and leads to
conflicting statements»), issue #975, #962, #639; `runTasteTool` @786510.
**СИМПТОМ**

- **Отката нет.** Единственный способ убрать правило — вручную отредактировать
  `.commandcode/taste/taste.md` (что промпт главного агента прямо запрещает: «NEVER EDIT
  OR WRITE TO TASTE FILES», @2052400) или удалить файл целиком. Issue #568 просит вернуть хотя бы удаление remote-пакетов.
- **Устаревание не лечится.** Мейнтейнер в #382 отвечает буквально: «taste will be
  learned; **it's your responsibility to judge it and remove what you think is not
  helpful, just like a PR review**». То есть lifecycle правила = ручной.
- **Нет команды «удалить правило».** `taste delete` **удалён из CLI** (CHANGELOG:
  «Remove non-functional `taste delete` command (remote deletion not implemented)»),
  `taste`-подкоманды = `push|pull|list|lint|open` (#568 подтверждает перечень).
- **Нет ни TTL, ни decay, ни «не подтверждено N сессий»** — `grep -i 'decay|expire|stale'`
  по taste-коду даёт 0 совпадений в механике taste (слова `stale` встречаются только в
  промптах про кэш/сессии).

**НАШ АНАЛОГ:** у нас **есть** откат — `rollback_proposal` (`crates/pa-core/src/refinement/planner.rs:505`):
он реплеит `applied_edits` в обратном порядке, восстанавливая `before`-снапшоты, а для
create-only edit генерирует `Delete`. Это **сильнее**, чем у Command Code.

**ЧТО ДЕЛАТЬ: сохранить наше преимущество и добавить недостающее — staleness.**
- Ввести `last_confirmed_at` и политику: entry, не подтверждённый за N сессий (или за
  M дней), помечается `Stale` и **перестаёт попадать в промпт-дайджест** (не удаляется —
  чтобы не терять данные).
- Авто-`rollback` по сигналу: если после применения refinement следующие K сессий дали
  `outcome: worse` (напр. больше ошибок tool-call, больше повторов) — применить
  `rollback_proposal` автоматически, с записью в `refinement_history.jsonl`.
- **Регрессионный тест:** (а) `rollback_proposal(result)` для `[create A, update B]` даёт
  `[delete A, update B→before]` в обратном порядке — у нас уже есть тест
  `apply_create_update_delete` в `planner.rs`, добавить симметричный на rollback;
  (б) entry с `last_confirmed_at` старше порога не рендерится в
  `format_harness_state_for_prompt` и не считается в `harness_digest_fingerprint`.

**СЕРЬЁЗНОСТЬ: высокая** (это то, чего у нас нет, а у них есть проблема, которую они не
решают).

---

## 3. Как taste влияет на выбор тула: где именно в промпте

**ID:** `taste-prompt-placement`
**ИСТОЧНИК:** `createSystemPromptBuilder` @923974; `renderTasteSection2` @913344;
`buildTasteGuidelinesSection` @2049577; `<file_operations>` @2041691; вызов сборки @647878.
**СИМПТОМ / ПРИЁМ**

Порядок секций системного промпта (жёсткий, @923974):

```
base (роль+тон+file_operations)  [cache:true]
  <instructions>  (memory/AGENTS.md)
  <workspace>
  <taste>          ← сюда вставляется файл целиком
  <skills>
  <explore_agent>
  <scratchpad>
  <env>
```

Секция `<taste>` = **два** блока: (1) `renderTasteSection2` — дословный контент
`taste.md` в маркерах `--- Content of .commandcode/taste/taste.md ---`; (2)
`<taste_guidance>` — ~3.4 КБ статической инструкции «что такое taste / как организован /
как пользоваться / CRITICAL RULES». Ключевые строки guidance:

- «Apply ALL relevant preferences to your work — **these are REQUIREMENTS, not
  suggestions**»
- «When you see "See [cli/taste.md]" for a CLI task, **IMMEDIATELY read**
  .commandcode/taste/cli/taste.md before writing any code»
- «**NEVER EDIT OR WRITE TO TASTE FILES** … managed automatically by the learning system»

Сборка — **на каждый запрос** (@647878, внутри agent loop перед `callModelWithRetry`),
файл перечитывается с диска, кэша нет. Позиция в промпте — **раньше** skills/scratchpad/env
и до истории; на секции стоит `cache:true` (`{text:o,cache:true}` для base и `{text:r.join,
cache:true}` для блока секций), поэтому правка taste **инвалидирует кэш префикса** —
то есть каждое фоновое обучение стоит полного перечитывания системного промпта.

**Важно:** «grep вместо find» **не** живёт в taste. Это `<file_operations>` в базовом
промпте (@2041691), дословно:

```
- Do NOT use Bash when a dedicated tool exists. Prefer:
  - read_file over cat/head/tail/sed for reading files
  - edit_file over sed/awk for editing files
  - glob over find/ls for file search
  - grep over shell grep/rg for content search
```

Это статический, всегда-включённый блок. Taste добавляет **поверх** него
проект-специфичные предпочтения. То есть у них **двухуровневая** система:
«универсальные правила выбора тула» (код/промпт продукта) + «выученные предпочтения
проекта» (taste).

**НАШ АНАЛОГ:** у нас в `system_prompt.rs:336-380` уже есть похожий блок («MUST use it for
definitions, references… NEVER text-search for code intelligence») и «## Working with these
tools». Continual-harness промпт-нота вставляется в **дайджест** (см. `harness_state` в
начале этой сессии) — это другой слой.

**ЧТО ДЕЛАТЬ: перенять структуру, а не текст.**
1. Разделить у нас **явно** два слоя: (а) «правила выбора тула» — статические, в
   `system_prompt.rs`; (б) «выученные предпочтения» — harness memory. Сейчас они
   смешаны: memory-запись «Prefer graph.search over grep» будет конкурировать с
   жёстким «MUST use lsp for definitions» без объявленного приоритета.
2. Добавить в промпт-дайджест строку приоритета (см. §2.3) — иначе выученное правило
   может перебить инвариант.
3. **Регрессионный тест:** промпт содержит ровно один блок «правила выбора тула» и он
   **до** дайджеста harness; при наличии memory-entry с текстом «prefer grep» промпт
   всё равно содержит «MUST use lsp for definitions» (инвариант не вытесняется).

**СЕРЬЁЗНОСТЬ: средняя.**

---

## 4. Repair-слои: точные имена, правила, порядок (проверено исполнением)

### 4.1 Полный конвейер (уточнённая последовательность)

```
tool-call из стрима
 ├─ coerceToolInput(raw, toolName, schema)                  @953940
 │    array[1] → arr[0]; null → {}; JSON-строка → parse;
 │    голая строка → wrapBareStringRoot (только если ровно 1 required И он string/array)
 │    number/bool root → {} (ПОТЕРЯ, не восстанавливается)
 ├─ canonicalToolUse → gatedToolName (только bash/Bash → shell_command)   @817207
 ├─ emit tool_queued (input — ОРИГИНАЛЬНЫЙ; name — канонический ТОЛЬКО для
 │     gateAsCanonical-алиасов, т.е. bash→shell_command; bash_output остаётся собой)
 ├─ checkPermissions  ← ДО alias/repair (@641300)
 │    deny по «нет такого тула» → denyMessage `No tool named "X" exists…` (policy)
 ├─ mods[].beforeToolCall  (rewrite input | block | result | terminate)   @638363
 ├─ toolRunner.execute                                     @821109
 │    ├─ resolveToolNameAlias (silent redirect + defaults + note)         @817370
 │    ├─ repairToolInput (12 правил, 2 прохода)                           @587715
 │    ├─ violations? → formatToolInputError → REJECT (тул не исполняется)
 │    ├─ tool.run(input)
 │    └─ truncate(25K) + applyRepairNotes(<repair_note>…)
 ├─ mods[].afterToolCall
 └─ emit tool_input_repaired (rulesFired, hintCount, receivedKeys)
```

**Точное место reject-а:** внутри `toolRunner.execute` (`createToolRunner.execute`,
@821109), **после** alias и repair. Reject — это `{ok:false, error, deferred, inputRejected:true,
repairs}`; `execGuarded` (@637425) превращает его в `isError:true` tool-result. Никакого
авто-ретрая нет.

### 4.2 Каталог правил: 12, не 13

**ID:** `repair-catalogue-12`
**ИСТОЧНИК:** `zf=[{name:"dropNullOrUndefinedField"` @1932672.
**Проверено:** в массиве ровно 12 элементов, в этом порядке (первое сработавшее выигрывает):

```
 1 dropNullOrUndefinedField      $f
 2 dropEmptyObjectPlaceholder    _f
 3 coerceStringToNumber          Ff
 4 coerceStringToBoolean         Nf
 5 parseJsonStringifiedArray     Bf
 6 parseJsonStringifiedObject    jf
 7 wrapBareScalarAsArray         (inline)
 8 stripMarkdownLinkFromPath     Gf
 9 stripOptionalMarkerFromKey    Kf
10 renameAliasedField            Of
11 rejectUnsupportedAction       Hf
12 dropUnknownKey                Uf
```

Наш отчёт говорит «13» и перечисляет 12 — исправить.

### 4.3 Двухпроходный sweep — то, чего в нашем отчёте нет

**ID:** `repair-two-pass-sweep`
**ИСТОЧНИК:** `runCatalogue` @587225.
**ПРИЁМ:** сначала sweep по `originalKeys`, затем **второй** sweep по ключам, появившимся
после репэйра. Это позволяет цепочкам: `include:"[\"a\",\"b\"]"` → `paths:["a","b"]`
(renameAliasedField, затем parseJsonStringifiedArray во втором проходе). Проверено:

```
{include:'["a.ts","b.ts"]'} → {paths:["a.ts","b.ts"]}   rules: renameAliasedField,parseJsonStringifiedArray
{include:'a.ts'}            → {paths:["a.ts"]}          rules: renameAliasedField,wrapBareScalarAsArray
```

Массив внутри массивов-объектов обрабатывается отдельным рекурсивным вызовом с
префиксом `key[i].` — но **только на один уровень** (вложенный объект внутри объекта
не чистится; проверено).

**НАШ АНАЛОГ:** `prepare_edit_arguments` (`crates/pa-core/src/tools/edit.rs:114`) —
одноразовая ручная трансформация (`edits`-строка → массив; `oldText/newText` →
`edits`). Один проход, три правила, нет общего каталога.

**ЧТО ДЕЛАТЬ: перенять двухпроходность.** При написании нашего `repair_tool_input`
сделать `for pass in 0..2 { for key in keys(pass) { apply_first_matching_rule() } }`.
**Регрессионный тест:** `{"cmd": "npm i", "timeout_ms": "60000"}` → оба поля
исправлены (alias + coerce) **за один вызов**; и `{"include": "[\"a\"]"}` →
`{"paths":["a"]}` (два правила в цепочке).

**СЕРЬЁЗНОСТЬ: средняя** (дешёвая страховка, ровно тот случай, где одно-проходный
ремонт оставляет «полу-починенный» вызов).

### 4.4 Второй контур валидации: `requiredAlternatives` и `exactlyOneOf`

**ID:** `schema-required-alternatives`
**ИСТОЧНИК:** `collectInputViolations` @584898; `read_file` schema @1849112; `sleep` @502045.
**СИМПТОМ / ПРИЁМ**

Помимо `required` есть два расширения схемы:
- `requiredAlternatives: {file_path: ["paths"]}` — «`file_path` обязателен, НО
  удовлетворяется наличием `paths`». Позволяет одному тулу иметь два
  взаимоисключающих способа задать цель без union-типа (union-типы ломают Gemini —
  это прямо объяснено в docs `tools.md`).
- `exactlyOneOf: [["seconds","until"]]` — «ровно один из группы».

Тексты reject-а:
```
• file_path: required field is missing            (когда нет ни того, ни другого)
• seconds / until: provide exactly one of these, but seconds and until were both sent
• seconds / until: provide exactly one of these, none was sent
```

**НАШ АНАЛОГ:** `validate_tool_arguments` (`crates/pa-agent/src/validation.rs:15`) знает
только `type`, `enum`, `const`, `properties`, `required`, `additionalProperties`,
`items` (кортежи и схемы). `requiredAlternatives`/`exactlyOneOf` **не поддерживаются** —
если наш тул объявит такое расширение, оно будет молча проигнорировано.

**ЧТО ДЕЛАТЬ: перенять (по мере надобности).** Для `edit` (5 режимов!) и `lsp`
(22 действия) это прямо релевантно: сейчас взаимоисключающие параметры, скорее всего,
валидируются вручную внутри `run()`.
**Регрессионный тест:** схема с `exactlyOneOf:[["a","b"]]`: `{}` → ошибка «none was
sent»; `{a:1,b:2}` → «both were sent»; `{a:1}` → ok. И `requiredAlternatives`:
`{paths:[...]}` при `required:["file_path"]` → ok; `{}` → «file_path: required».

**СЕРЬЁЗНОСТЬ: средняя.**

### 4.5 `<repair_note>` — механика и её дефект

**ID:** `repair-note-mechanics`
**ИСТОЧНИК:** `withRepairNotes` @582556; `applyRepairNotes` @820229; `extractRepairNotes` @582612;
`Af=/^<repair_note>([\s\S]*?)<\/repair_note>$/` @1924107.
**ПРИЁМ:** заметки идут **префиксом** к тексту результата, каждая на своей строке:

```
<repair_note>Renamed `path` to `file_path` for tool "edit_file". …</repair_note>
<repair_note>Coerced string `limit` to a number.</repair_note>
Edited /a.ts (1 replacement)
```

`extractRepairNotes` снимает их **только с начала строки** и **только пока идут подряд**
(`break` на первой не-заметке) — используется в TUI, чтобы не показывать их человеку.
Дефект: **заметки остаются в истории для модели навсегда** — при повторной подаче в
историю они не вырезаются (функция `stripRepairNotes` отсутствует, `grep` даёт 0).
То есть отрепэйренный вызов оставляет в контексте навсегда «учебник», и при 30 репэйрах
это уже тысячи токенов мусора.

**НАШ АНТИПОД:** у нас ничего подобного нет.

**ЧТО ДЕЛАТЬ: перенять с исправлением.**
- Добавлять `<repair_note>` **только к последнему tool-result** и **вырезать** заметки
  из более старых сообщений при сборке контекста (у нас это делается в
  `prepare_for_send`-аналоге; у них такой шаг есть для orphan-heal, но не для заметок).
- **Регрессионный тест:** после 3 репэйренных вызовов в контексте присутствует ровно
  одна `<repair_note>` (от последнего); после успешного «чистого» вызова — ноль.

**СЕРЬЁЗНОСТЬ: средняя** (приём сильный, реализация у них с утечкой).

### 4.6 Алиасы имён тулов: таблица целиком

**ID:** `tool-name-alias-table`
**ИСТОЧНИК:** `kC={bash_output:{to:"shell_output"}…` @2008594.

```js
bash_output         → shell_output
task_output         → shell_output  (defaults:{wait:"exit"} — если block !== false)
tool_search         → search_tools
read_multiple_files → read_file
bash  /  Bash       → shell_command  (gateAsCanonical: true — для permission-гейта
                                      считается каноничным именем)
```

Всего 6 записей. Тонкость, которой нет в нашем отчёте: в `createToolRunner.execute`
(@821109) после алиасинга стоит **fallback на оригинальное имя** —
`g = byName(alias.toolName); h = g ?? byName(originalName)`. То есть если целевого тула в
сессии нет (например, `read_file` скрыт моделью/режимом), вызов пойдёт по **исходному**
имени, а repair-note всё равно будет добавлена (`x = [{key:n, fix:S, rule:"renameAliasedTool"}]`)
— модель получит совет «call `read_file` directly» при том, что `read_file` ей недоступен.
Это ещё один класс «полезный совет в пустоту».

`gateAsCanonical` — тонкость: `gatedToolName` (@817207) применяется
**до** permission-проверки, поэтому правило `Bash(...)` в settings матчится как
`shell_command`. `defaults` подставляются только если поля нет (или оно `null`), с
исключением: `wait` не подставляется, если `block === false` (`blockCancelsWait`).

**Найденный баг (проверено):** alias `task_output → shell_output` с `defaults:{wait:"exit"}`
подставляет `wait` даже если модель явно передала `block:true` — потому что
`blockCancelsWait` смотрит только `block === false`, а `true` не отменяет default.
То есть `task_output{block:true}` (семантика «ждать выхода») получает `wait:"exit"`
случайно правильно, а `task_output{block:false}` — правильно не получает. Но
`task_output{block:"false"}` (строка!) — **получит** `wait:"exit"` и заблокируется,
хотя модель просила peek. Проверено: `resolveToolNameAlias({task_output,{block:'false'}})`
→ `{wait:"exit"}` (default применён, строка не распознана как false).

**Важно:** из 6 записей только `task_output` имеет реальную схему в бандле (retired,
`visible:()=>false`, @1890669); `bash_output`, `read_multiple_files`, `tool_search`
схем **не имеют** — это чистые строковые редиректы для исторических вызовов из старых
сессий. То есть alias-таблица у них — не «удобство», а **механизм обратной
совместимости через компакцию и рестарт**: старая история с `bash_output` должна
исполниться в новой версии продукта.

**Следствие для permission-гейта (важно, в нашем отчёте нет):** порядок такой —
`canonicalToolUse` (@642432, только для `gateAsCanonical`) → `checkPermissions` →
`execute` (alias → repair). Значит:
- `bash` проверяется как `shell_command` (правило `Bash(...)` матчится);
- `bash_output` проверяется как **`bash_output`**, хотя исполнится как `shell_output` —
  правило на `shell_output` **не сработает**, а `deny`-правило на `shell_output`
  обходится вызовом `bash_output`. Это дыра в permission-политике: гейт видит
  до-алиасное имя.

**НАШ АНАЛОГ:** `accepts_alias` (`crates/pa-agent/src/types.rs:551`,
`crates/pa-core/src/session_engine/tool_bridge.rs:72`) — реализован **только** для
`python_repl`: `matches!(name, "python_repl" | "python" | "ipython")`
(`crates/pa-types/src/ai/mod.rs:256`). Таблицы редиректов с переносом аргументов **нет**;
`find_tool` (`crates/pa-agent/src/agent_loop/tools.rs:32`) умеет только точное имя +
`accepts_alias`, и **аргументы не переписывает** — `prepare_arguments` вызывается уже
после выбора тула (`tool_call.rs:61`).

То есть у нас алиас работает только как «тот же тул, другое имя», а у них — как
«другой тул + перенос аргументов + подстановка defaults + repair-note».

**ЧТО ДЕЛАТЬ: перенять таблицу (у нас 4 тула → 2–3 записи) + исправить их баг.**
Записи: `graph_search → graph{action:"search"}`, `lsp_symbols → lsp{action:"symbols"}`,
`edit_replace → edit{mode:"replace"}`, `python → python_repl`.
**Регрессионный тест:** `resolve_alias("graph_search", {query:"x"})` →
`("graph", {action:"search", query:"x"})` + repair_note «call `graph` directly»;
и отдельно — `block:"false"` (строка) не должна превращаться в блокирующий default
(у них это баг; у нас должно быть `coerce_bool` **до** проверки defaults).

**СЕРЬЁЗНОСТЬ: средняя.**

### 4.7 `coerceToolInput`: что теряется

**ID:** `coerce-root-losses`
**ИСТОЧНИК:** `coerceToolInput` @953940; `wrapBareStringRoot` @953300.
**Проверено:**

| вход | результат | потеря |
|---|---|---|
| `[{...}]` | `{...}` | — |
| `[]` | `{}` + `recovered:false` | да, пусто |
| `null` | `{}` + `rawType:"null"` | да |
| `'{"a":1}'` | `{a:1}` | — |
| `'not json'` (1 required string) | `{param:"not json"}` | — |
| `'not json'` (3 required) | `{}` | **да, весь текст** |
| `42` / `true` | `{}` | **да** (number/bool root не обрабатывается) |

Телеметрия `tool_input_coerced` (@649679) отправляется **только** для array/null-корней
(`s||a` в условии возврата), для number/bool — молча.

**НАШ АНАЛОГ:** в `validation.rs` коэрция работает **внутри** схемы (строки→числа и т.п.),
но корень аргументов не нормализуется вообще: `serde_json::from_value::<T>` на
`[{...}]` даст ошибку «invalid type: sequence».

**ЧТО ДЕЛАТЬ: перенять + расширить.** Нормализовать корень: массив длины 1 → элемент;
`null`/отсутствие → `{}`; JSON-строка → parse; **число/бул** → `{}` **с событием**
(не молча). И **всегда** эмитить событие коэрции, а не только для array/null.
**Регрессионный тест:** `raw=[{...}]` → ok; `raw=[]` → reject с «No arguments were
received at all»; `raw="42"` → reject + событие `tool_input_coerced{recovered:false}`.

**СЕРЬЁЗНОСТЬ: средняя.**

---

## 5. Известные баги repair: ложные срабатывания и потеря аргументов

### 5.1 `dropUnknownKey` молча съедает аргументы, которые тул **читает**

**ID:** `drop-unknown-key-silent-loss`
**ИСТОЧНИК:** `Uf` @1930040 + `grep.run` @1801650 + `vision` @594477 + `task_stop` schema @1890669.
**Проверено (4 независимых случая, см. также §5.4):**

| тул | что модель прислала | что стало | что делает `run()` |
|---|---|---|---|
| `grep` | `{pattern:"x", include:["*.ts"]}` | `{pattern:"x"}` | **читает `e.include`** — но он уже удалён |
| `vision` | `{question:"q", image:"/tmp/shot.png"}` | `{question:"q", image_index:"/tmp/shot.png"}` | `image_index` ожидает number → путь теряется, индекс = NaN |
| `task_stop` (@1890669) | `{id:"s0abc"}` | `{}` | `run()` читает `taskId`→`task_id`→`id`→`shell_id` — но в alias-таблице `kC` `task_stop` **нет** (есть только `kill_shell`) |

Три разных корня одной болезни: **схема и рантайм-читатель расходятся**, а
`dropUnknownKey` (правило №12, последнее) довершает — удаляет то, что `run()` умеет
прочитать. `vision.image → image_index` — ещё и type-confusion: алиас ведёт на поле
**другого типа** (string-путь → number-индекс).

**НАШ АНАЛОГ:** `validate_tool_arguments` **не удаляет** лишние ключи, если в схеме нет
`additionalProperties:false`. У нас это значит: лишние ключи проходят валидацию и
игнорируются тулом — **лучше**, чем молчаливое удаление (модель хотя бы видит свой
input в истории). Но у нас есть обратная проблема: если `prepare_arguments`
(`edit.rs:114`) перепутает ключи — валидация промолчит.

**ЧТО ДЕЛАТЬ: не перенимать `dropUnknownKey` в текущем виде.**
1. Если поле неизвестно схеме, но **известно `run()`** — это баг схемы, а не модели.
   Ввести **гард-тест**: множество ключей, которые `run()` читает, ⊆ множество ключей
   схемы. У них это нарушено трижды; у нас `graph.rs`/`lsp` читают params через
   `serde_json::from_value` в типизированную структуру, поэтому такие расхождения
   проявятся как ошибки десериализации — но **только** если поле не `Option`+`default`.
2. Если всё же вводим drop-unknown: **никогда** не дропать ключ, который непустой и
   похож на алиас (эвристика: расстояние Левенштейна ≤2 до известного ключа) —
   вместо дропа reject с подсказкой.
3. **Регрессионный тест (обязательный):** для каждого нативного тула — «все ключи,
   читаемые в `execute`, объявлены в `parameters()`»; и «алиас не меняет тип поля»
   (alias-таблица с типами: `image(string) → image_index(number)` должна быть
   **запрещена** статически).

**СЕРЬЁЗНОСТЬ: высокая** (это класс «тихая потеря аргументов», который ломает
поведение без единой ошибки в UI).

### 5.2 Ложное срабатывание алиаса при наличии канонического поля

**ID:** `alias-false-positive`
**ИСТОЧНИК:** `Of` (renameAliasedField) @1928110.
**Проверено:** `{file_path:"/a.ts", path:"/b.ts", old_string, new_string}` →
`{file_path:"/a.ts", old_string, new_string}` + **`dropUnknownKey` на `path`**. То есть
модель прислала **два разных пути**, один из них потерян молча — без reject, без
предупреждения. Правило защиты в коде есть (`if (void 0 !== s[o] && "" !== s[o]) continue;`
— не перезаписывать существующее каноническое), но **следующее** правило (dropUnknownKey)
всё равно удаляет конфликтующий ключ.

Второй случай: `{old:"AAA", oldValue:"BBB"}` → `old_string:"AAA"`, `oldValue` дропнут.
Побеждает **первый по порядку ключей**, а не «более вероятный» — то есть исход зависит
от порядка полей в JSON (у моделей он произволен).

**НАШ АНАЛОГ:** нет алиасов — нет бага. Но при введении (см. §4.6) он появится.

**ЧТО ДЕЛАТЬ: при введении алиасов — reject на конфликт.**
Правило: если и канонический ключ, и алиас присутствуют **с разными непустыми
значениями** → reject с сообщением «both `file_path` and `path` were sent with different
values; resend with only `file_path`». Не «побеждает первый».
**Регрессионный тест:** `{file_path:"/a", path:"/b"}` → reject; `{file_path:"/a", path:"/a"}`
→ ok (совпадающие значения безвредны).

**СЕРЬЁЗНОСТЬ: высокая** (тихая потеря + недетерминизм).

### 5.3 `repairToolInput` падает на неполной схеме (TypeError, не reject)

**ID:** `repair-throws-on-sparse-schema`
**ИСТОЧНИК:** `collectInputViolations` @584898 (`e.schema.input_schema.required` → `.flatMap`),
`runCatalogue` @587225 (`Object.keys(e.properties)`).
**Проверено:**

```
schema {type:"object"}                              → TypeError: Cannot convert undefined or null to object
schema {type:"object", properties:{a}}              → TypeError: Cannot read properties of undefined (reading 'flatMap')
schema {type:"object", properties:{}, required:[]}  → ok
```

`execGuarded` ловит это в `catch` (@637900) и превращает в `errorText3(e)` — то есть
модель получает **`Cannot read properties of undefined (reading 'flatMap')`** вместо
коррекции. Это реальный класс: MCP-тулы приходят с чужими схемами
(`mcpToolDefToSchema` @4094 **всегда** ставит `required:[...]`, но `properties` может
быть `{}`, а сторонние тулы — что угодно).

**НАШ АНАЛОГ:** `validate_tool_arguments` терпим к отсутствию `properties`/`required`
(`schema.get(...)` → `if let Some`), падения нет.

**ЧТО ДЕЛАТЬ: не перенимать; зафиксировать как антипаттерн.**
**Регрессионный тест:** `validate_tool_arguments("x", {"type":"object"}, {})` → `Ok`,
не panic; и `{"type":"object","properties":{"a":{"type":"string"}}}` (без `required`) → `Ok`.

**СЕРЬЁЗНОСТЬ: низкая у нас, но полезна как регресс-щит при портировании их кода.**

### 5.4 MCP-схемы: `additionalProperties:true` игнорируется → все аргументы удаляются

**ID:** `mcp-additional-properties-dropped`
**ИСТОЧНИК:** `mcpToolDefToSchema` @4094 + `dropUnknownKey`.
**Проверено:** схема `{type:"object", properties:{}, required:[], additionalProperties:true}`
(типичная для MCP-тула без объявленных параметров) + input `{foo:1,bar:"baz"}` →
**`{}`**. Все аргументы удалены. `additionalProperties:true` при этом **сохраняется** в
схеме (спред `...c` в `mcpToolDefToSchema`), но `dropUnknownKey` его не читает.

Это ровно та механика, что в issue #1025 («MCP tool calls: arguments truncated/fail to
parse in transit — empty `{}` args, required fields missing») выглядит как «аргументы
потерялись в транзите». Аргументы не теряются в транзите — их **удаляет репэйр**.

**НАШ АНАЛОГ:** MCP-тулы у нас (`crates/pa-core/src/mcp/`) — схемы приходят извне;
`validate_tool_arguments` уважает `additionalProperties` — проверка только
`== Some(false)` (`crates/pa-agent/src/validation.rs:220`), плюс коэрция по
`additionalProperties`-схеме при объектном значении (`:113`). Лишние ключи **не**
удаляются. Бага нет.

**ЧТО ДЕЛАТЬ: не перенимать; добавить тест-щит.**
**Регрессионный тест:** `validate_tool_arguments("mcp__x", {type:"object",
properties:{}, additionalProperties:true}, {foo:1})` → `Ok({foo:1})`; и
`additionalProperties:false` → ошибка «Property is not allowed».

**СЕРЬЁЗНОСТЬ: высокая как знание** (это корневая причина целого класса issue),
**нулевая как работа** (у нас уже правильно).

### 5.5 Дубликат правила при повторной записи — «added» вместо «upgraded»

**ID:** `taste-reword-duplicates`
**ИСТОЧНИК:** `compareLearnings` @782208 (ключ = точный текст).
**Проверено:** `"Use pnpm."` → `"Use pnpm for installs."` даёт `added:[new]`, старое
правило **остаётся** в файле. Через N сессий в taste.md живут обе формулировки, и модель
видит два конкурирующих правила.

**НАШ АНАЛОГ:** `RefinementEdit` адресуется по `id` (`planner.rs:193 validate_edit`),
поэтому переформулировка — это `Update` того же id, дубликата не возникает.

**ЧТО ДЕЛАТЬ: перенять урок, не код.** При создании memory-entry считать
нормализованный текст (lowercase, убрать пунктуацию) и **отвергать create**, если
нормализованный текст совпадает с существующим (требовать update по id).
**Регрессионный тест:** create с текстом, отличающимся только регистром/пунктуацией от
существующего → `Err(Duplicate{existing_id})`.

**СЕРЬЁЗНОСТЬ: средняя.**

---

## 6. beforeToolCall mods: переписать input, подсунуть результат

**ID:** `before-tool-call-mods`
**ИСТОЧНИК:** `executeOne` @638363; `execute` @821109; docs `mod-builder/reference/hooks-and-events.md`.
**ПРИЁМ (проверено чтением кода + официальным reference)**

`beforeToolCall({toolCallId, toolName, input, state}, ctx)` может вернуть **любое** из:

| возврат | эффект |
|---|---|
| `{input: newInput}` | **переписать аргументы**; цепочка модов: output предыдущего = input следующего; исполнение идёт с **последним** input |
| `{result: content}` | **подменить результат** — тул **не исполняется** (`p = r.result; break`), `isError:false` |
| `{block:true, additionalContext}` | заблокировать; текст уходит как **error**-tool-result; события `tool_running/completed` **не** эмитятся |
| `{additionalContext}` | добавить контекст (уходит в `hookContexts` сообщения) |
| `{terminate:true}` | остановить сессию |

Тонкости, которых нет в нашем отчёте:
- `tool_queued` эмитится **до** модов и всегда с **оригинальным** input (имя может быть
  уже канонизировано для `bash`/`Bash`) — UI показывает то, что просила модель;
  перезапись видит только исполнение. Это правильный принцип
  наблюдаемости, стоит перенять.
- `{result}` **прерывает цикл модов** (`break`) — последующие моды не видят вызов.
- Ошибка в моде → событие `mod_error{modId, hook:'beforeToolCall'}` и **вызов
  продолжается** (не fail-closed).
- **Баги:** issue #852 — `beforeToolCall` **не вызывается** для tool-call'ов
  sub-агентов (только permission-правила); issue #863 — `addTool` глобальный, нет
  per-agent scoping.

**НАШ АНАЛОГ:** `BeforeToolCallResult` (`crates/pa-agent/src/types.rs:629`) — это ровно
два поля: `{block: bool, reason: Option<String>}`. `BeforeToolCallContext`
(`types.rs:649`) даёт `{assistant_message, tool_call, args, context}`, но:
- **переписать input нельзя** — `PreparedToolCall` берёт `tool_call.clone()`
  (`tool_call.rs:96`), то есть оригинал, а не `validated_args`;
- **подменить результат нельзя** — поля нет;
- `additionalContext` нет.
То есть из пяти возможностей реализована **одна** (`{block}`). `AfterToolCallResult`
(`types.rs:637`) при этом богаче их `afterToolCall`: есть `content`/`details`/`is_error`/
`terminate`, но нет `modState` и нет `additionalContext`.

**ЧТО ДЕЛАТЬ: перенять `{input}` и `{result}`.**
1. Расширить `BeforeToolCallResult` (`types.rs:629`) до `{block, reason,
   input: Option<Value>, result: Option<AgentToolResult>,
   additional_context: Option<String>}`; при `input` —
   **пере-валидировать** аргументы после подмены (у них это делается один раз **до**
   модов — то есть мод может подсунуть невалидный input, и тул упадёт внутри `run()`;
   это их баг, не повторять).
2. `{result}` — да, но с обязательным `details.synthetic: true`, чтобы телеметрия
   отличала синтетику.
3. Порядок «мод видит **сырой** вызов до детерминированного репэйра» (у них) — у нас
   наоборот: `before_tool_call` получает **уже провалидированные** `args`. Для наших
   целей (taste-кандидат из ошибки модели) нужен именно **до-репэйр** вариант: иначе
   мод не увидит саму ошибку. **Добавить второй хук** `before_repair` (или передавать
   и `raw_arguments`, и `validated_args`).
4. **Регрессионный тест:** мод `{input:{...}}` → `execute` получает новый input и
   он проходит `validate_tool_arguments`; мод `{result}` → `run()` не вызывается
   (счётчик вызовов = 0), `is_error=false`, `details.synthetic=true`; мод, вернувший
   невалидный input → reject, а не паника внутри тула.

**СЕРЬЁЗНОСТЬ: высокая** (это единственный способ реализовать «арбитр переписал вызов»
без перезапроса модели).

---

## 7. Прочие найденные приёмы, которых нет в нашем отчёте

### 7.1 Orphan-heal: синтетический tool_result для «висящего» tool_use

**ID:** `orphan-tool-result-heal`
**ИСТОЧНИК:** `prepareForSend` @632676; `Wv="No result — the tool call did not complete
(interrupted or lost)."` @1959351.
**ПРИЁМ:** перед каждой отправкой модели сообщения нормализуются: для каждого
`tool_use` без парного `tool_result` **дописывается** синтетический результат с текстом
`No result — the tool call did not complete (interrupted or lost).`; пустые текстовые
блоки вычищаются; соседние сообщения одной роли **склеиваются**; мета снимается.
Это лечит 400-е «tool_use without tool_result» после прерывания/компакции.

**НАШ АНАЛОГ: есть, и он даже лучше по тексту — но неполный.**
`crates/pa-ai/src/providers/transform_messages.rs:176-195` (`insert_synthetic_tool_results`)
уже дописывает `Message::ToolResult` с `is_error: true` и текстом **`"No result provided"`**
для неотвеченных вызовов; тесты `inserts_synthetic_tool_results_for_unanswered_calls`
(@:313) и `drops_errored_assistant_turns` (@:340) на месте. Чего **нет** у нас и есть у них:
1. **слияние соседних сообщений одной роли** (`mergeSameRole` в `prepareForSend`) — без него
   после инжекта синтетики в истории появляются два подряд `user`, что ломает часть провайдеров;
2. **вычищение пустых текстовых блоков** (`isEmptyText` → `filter`) — пустой `text:""`
   в ассистентском сообщении даёт 400 у Anthropic;
3. **текст-объяснение**: `"No result provided"` не говорит модели, **почему** результата нет
   и что делать. Их формулировка («interrupted or lost») объясняет причину — это разница
   между «непонятная дыра» и «меня прервали, продолжу иначе».

**ЧТО ДЕЛАТЬ: дозакрыть, а не перенимать с нуля.**
1. Заменить текст на объясняющий: `"No result — the tool call did not complete (interrupted
   or lost). Re-issue the call if you still need it."`
2. Добавить шаг `merge_adjacent_same_role` и `drop_empty_text_blocks` в тот же проход
   (сейчас их нет — `grep` по `mergeSameRole|isEmptyText` в `crates/` даёт 0).
3. **Регрессионный тест (вход → ожидание):** `[user, assistant{tool_use A}, user("again")]`
   → после трансформа: `[user, assistant, tool_result{A, is_error:true, "No result — …"},
   user("again")]` и **нет** двух подряд `user`; `assistant{text:""}` → блок выброшен.

**СЕРЬЁЗНОСТЬ: средняя** (у нас 60 % уже есть; дозакрытие — 20 строк + 2 теста).
### 7.2 Truncation: «не режь мысль» — отказ вместо обрезки

**ID:** `truncate-refuse-not-slice`
**ИСТОЧНИК:** `truncateToolText` @561740; `nf="Results are too large. Let's try a more
specific pattern to narrow it down. There's a 25K token limit on tool outputs per request."`
@1912890; `spilledNotice` @561400.
**ПРИЁМ:** при превышении 25K токенов вывод **не обрезается посередине** — он
**заменяется** сообщением «слишком большой, сузь запрос», а полный вывод сохраняется в
spill-файл, путь к которому **вписан в тот же текст**:

```
[... output truncated: 84,213 chars total, showing first 20,000 (25K token limit on tool
outputs per request) ...]
[full output saved to: /root/.commandcode/spill/<...>.txt — read it with read_file
(offset/limit) or grep]
```

Исключения (`tf = {web_search, web_fetch, read_file}`): эти тулы сами себя ограничивают
и не подменяются. MCP-тулы (`mcp__*`) **не** обрезаются вовсе (их вывод — чужой контракт).

**НАШ АНАЛОГ: есть, но в другой парадигме.** `crates/pa-core/src/tools/truncate.rs` —
два лимита (2000 строк / 50 КБ), «never returns partial lines»; spill-механика есть только
у bash (`crates/pa-tui/src/bash_bang.rs:185` — `[Output truncated. Full output: /tmp/spill.log]`),
и она живёт в **TUI-рендерере**, не в общем слое. То есть:
- у нас обрезка — **линейная** (по строкам/байтам), у них — **отказ с инструкцией**;
- у нас spill-путь не попадает в текст для **модели** (только в карточку для человека);
- у нас нет per-tool исключений (read_file у нас тоже режется общим слоем).

**ЧТО ДЕЛАТЬ: перенять.**
1. Добавить в `TruncationResult` вариант «refused»: при превышении **токен-бюджета**
   (25K) возвращать не хвост, а `nf`-подобный текст + `spill_path`.
2. Исключить из общего слоя тулы, которые сами себя ограничивают (`read_file`,
   `graph`, `lsp` — у них есть `max_lines`/`limit`).
3. Формулировку сделать **инструкцией**: «Results are too large. Narrow the pattern, path,
   or limit and call again» + путь к spill.
**Регрессионный тест:** вывод 84 КБ при бюджете 25K токенов → текст содержит
`full output saved to:` и путь; **не** содержит первых 20 КБ содержимого; повторный
`read_file(spill_path)` возвращает полный текст. И: `read_file` на 30 КБ **не**
подменяется отказом (сам режет по `limit`).

**СЕРЬЁЗНОСТЬ: средняя** (у нас обрезка «тихая»: модель видит обрезанный хвост и не знает,
что он обрезан, если не заметит маркер).
### 7.3 Continuation repair: три вида обрыва, три разных промпта

**ID:** `continuation-repair-3-kinds`
**ИСТОЧНИК:** `createContinuationProvider` @811008; тексты @2005244-@2005914.
**ПРИЁМ:** три независимых счётчика с разными лимитами и **разными** промптами:

| kind | триггер | лимит | промпт |
|---|---|---|---|
| `length` | `stopReason==="max_tokens"` | 3 | «cut off by the output token limit … Continue exactly where you left off — do not repeat content you already produced.» |
| `intent` | текст кончается на «повисшей интенции» (`endsWithDanglingIntent`: `:` в конце, или последняя фраза матчит `sC` — «let me / I'll / I'm going to …») | 2 | «You ended your turn right after saying you were about to do something, without doing it. **Do it now — make the tool calls** … do not restate the plan.» |
| `empty` | нет видимого контента | 2 | «Please continue your response. Along with thinking, also tell the user what is happening…» |

Счётчики `length`/`intent` сбрасываются, если в turn'е были tool-calls (`onTurnEnd:
hadToolCalls → a=0,l=0`). Refusal → notice и **стоп** (не ретраится).

**Наш отчёт** описывает это как «до 3 / до 2 / до 2» — верно, но **не приводит тексты**,
а они тут главное: `intent`-промпт буквально требует «сделай tool call, не пересказывай
план». Это самая ценная часть.

**НАШ АНАЛОГ: хук есть, детектора нет.** `AgentLoopConfig.get_continuation_messages`
(`crates/pa-agent/src/agent_loop/mod.rs:110`) — точка расширения уже проведена и вызывается
в `run.rs:217-239`, но **ни одной реализации** в дереве нет (`grep get_continuation_messages`
по `crates/pa-core` — 0 совпадений): крючок висит пустым. `endsWithDanglingIntent`-аналога
и трёх счётчиков нет вовсе.

**ЧТО ДЕЛАТЬ: заселить уже существующий хук.** `get_continuation_messages` — идеальное
место; нужен `ContinuationProvider` с тремя счётчиками и `ends_with_dangling_intent`
(порт `sC`-регекспа: `^(?:(?:now|next|first|then|so|ok|alright|great|perfect|good)[,.!]?\s+)*
(?:let me|let's|i'll|i will|i'm going to|i am going to|we'll|we will)\b(?!\s+(?:not|never|…))`).
**Регрессионный тест:** ответ «Let me check the config files.» (без tool-call) →
эмитится continuation с текстом, содержащим «Do it now — make the tool calls»;
ответ «Done. Tests pass.» → continuation **не** эмитится; ответ «Here's the plan:» →
эмитится (кончается на `:`).

**СЕРЬЁЗНОСТЬ: высокая** (это прямой фикс «агент рассказал вместо того, чтобы сделать» —
issue #1001 ровно про это).

### 7.4 Deferred tools + `search_tools` и «не повторяй загрузку»

**ID:** `deferred-tools-and-loop`
**ИСТОЧНИК:** `createToolCatalog` @555769; `createSearchToolsTool` @817938;
`buildDeferredToolsPrompt`/`loadableSection` @559600; issue #937 (open).
**ПРИЁМ:** 19 тулов помечены `shouldDefer:true` (`grep -c 'shouldDefer:!0'` = 19; из них 11 через хелпер `deferred(tool, hint)`) — их
схемы **не** отправляются, вместо них в промпте блок «# Additional tools» с
**сигнатурой в TypeScript-нотации**:

```
- name{field: type, opt?: type}: first sentence of description
```

Инструкция к блоку дословно: «Each entry below reads `- name{arguments}: summary`, where
`{arguments}` is the tool's input schema WRITTEN AS A TYPESCRIPT OBJECT TYPE. Read it as
TypeScript: every field is REQUIRED unless its name ends in `?` … A `?` is notation —
never send it as part of a field name.» (@1912348) — последняя фраза закрывает ровно тот
баг, который иначе возникает: модель копирует `?` в имя поля (у них для этого есть и
отдельное правило `stripOptionalMarkerFromKey`).

Лимиты рендера: 160 симв. на поле, 120 симв. на первое предложение описания, `…` при
переполнении. Сигнатура тула **сжимается по бюджету**: поля добавляются, пока сумма
≤160, иначе `{a, b, …}`.

**Баг (issue #937, open):** повторный `search_tools` с тем же запросом возвращает **тот
же полный schema-блок** — нет memoизации, нет «already loaded». Модель зацикливается
(~70 раз в реальной сессии). Issue #936 — тот же отчёт, закрыт как дубликат #937.

**НАШ АНАЛОГ:** у нас 4 тула — deferred не нужен. Но **формат сигнатуры** полезен
для компактного рендера `parameters()` в системном промпте и в harness-дайджесте:
TypeScript-нотация в 3–4 раза короче JSON Schema при том же информационном содержании.

**ЧТО ДЕЛАТЬ: не перенимать механику; перенять формат + защиту от `?`.**
1. Функция `render_signature(schema) -> String` (порт `buildToolSignature`).
2. В инструкции к блоку — явно «`?` is notation, never send it».
3. Если когда-нибудь введём lazy-загрузку схем — **сразу** мемоизировать: повторный
   запрос того же имени возвращает `"already loaded, no changes"`, а не полную схему.
**Регрессионный тест:** `render_signature(graph.parameters)` ≤ 200 симв. и содержит `?`
у необязательных; рендер идемпотентен; повторный `load_tool(name)` дважды → второй ответ
содержит «already loaded».

**СЕРЬЁЗНОСТЬ: низкая.**
### 7.5 `taste lint` — реальный валидатор формата (8 диагностик + 12 подсказок)

**ID:** `taste-lint-rules`
**ИСТОЧНИК:** `validateTasteFile` @1421347; `validateTasteLearningLine` @1417443.
**Проверено (`/tmp/cc_verify/c19.cjs`):**

| вход | вердикт |
|---|---|
| `- Always run tests.` (без Confidence) | **error** «Learning is missing "Confidence:" field» |
| `Confidence: 1.5` | **error** «Confidence value 1.5 is out of range» |
| правило до первого `# ` | **error** «Learning found outside of any category» |
| `# workflow` без правил | warning «Category 'workflow' has no learnings» |
| `confidence:` в нижнем регистре | **valid** (lint регистронезависим — а runtime-парсер нет, см. §2.1) |
| пустой файл | error «File is empty» |
| многострочное правило (продолжение с отступом) | valid (склеивается в одну строку) |

**НАШ АНАЛОГ:** нет валидатора harness-состояния (структура типизирована, но
семантических инвариантов — «entry не пустой», «kind валиден» — нет явной проверки).

**ЧТО ДЕЛАТЬ: перенять как идею `harness lint`.** У нас дешевле: `validate_harness_state()`
проверяет (а) `title`/`content` непусты, (б) `path` внутри разрешённого корня,
(в) `reference`/`arguments` соответствуют контракту kind, (г) нет двух entry с
одинаковым `(kind, normalized(title))`.
**Регрессионный тест:** entry с пустым `content` → ошибка; два memory с одинаковым
title в разном регистре → ошибка дубликата.

**СЕРЬЁЗНОСТЬ: средняя.**

---

## 8. Схема описания тула: конкретные числа для нашей реализации

Замеры, которые стоит зафиксировать как бюджет (иначе описания расползутся):

| метрика | Command Code | предлагаем у нас |
|---|---|---|
| длина `description` тула | 139–2899 симв. (медиана ~950) | 600–2000, **≥800** |
| длина описания поля | 40–600 симв. | ≤ 200 |
| «When NOT to use» | есть у 1 из 30 (только `todo_write`) | **обязательно** у всех 4 |
| «Example call» | есть у 1 из 30 (`todo_write`) | **обязательно** у всех 4 |
| deferred-сигнатура | 160 симв. на поле, 120 на описание | не нужно (4 тула) |
| лимит tool-output | 25 000 токенов | сверить с нашим `truncate.rs` |
| лимит repair-заметок в контексте | нет лимита (утечка) | **1** (только последний вызов) |

---

## 9. Пять-десять самых важных пунктов

1. **`dropUnknownKey` — корень класса «тихая потеря аргументов».** Четыре подтверждённых
   случая в самом бандле (`grep.include`, `vision.image`, `task_stop.id`,
   MCP `additionalProperties:true` → `{}`, issue #1025). У нас бага нет (наш
   `validate_tool_arguments` не удаляет ключи); **не вводить** drop-unknown без гарда
   «ключи схемы ⊇ ключи, читаемые `run()`» и без reject на конфликт алиасов.

2. **Alias-таблица с переносом аргументов — приём, которого у нас нет вообще.**
   6 записей у них; нам нужно 3–4 (`graph_search`, `lsp_symbols`, `edit_replace`,
   `python`). Обязательные условия: (а) **reject** на конфликт «канонический ключ +
   алиас с разными значениями»; (б) алиас **не меняет тип** поля (их `image(string) →
   image_index(number)` — баг); (в) repair-note не советует тул, которого нет в сессии.

3. **Двухпроходный repair-sweep.** Проверено (`c21`/`c22`): `{cmd, timeout:"60000"}`
   требует двух проходов — alias в первом, coerce во втором. Одно-проходный ремонт
   оставляет «полу-починенный» вызов, и порядок правил становится значимым.

4. **`beforeToolCall` должен уметь `{input}` и `{result}`.** У нас
   (`tool_call.rs:75`) реализован только `{block}`. Без `{input}` невозможен «арбитр
   переписал вызов без перезапроса модели»; без `{result}` — синтетические ответы.
   И **обязательно** отдавать хуку **сырой** вызов, иначе арбитр не увидит ошибку модели.

5. **`<repair_note>` — сильный обучающий приём, у них с утечкой.** Перенимать с
   правилом «только последний tool-result; из старых вырезать».

6. **`intent`-continuation repair.** У нас хук `get_continuation_messages` уже проведён,
   но **пуст** (0 реализаций). Порт детектора «повисшей интенции» + промпт «Do it now —
   make the tool calls, do not restate the plan» — прямой фикс их issue #1001.

7. **Orphan-heal у нас уже есть** (`transform_messages.rs:176`), но не хватает трёх
   деталей: слияния соседних сообщений одной роли, вычищения пустых text-блоков и
   объясняющего текста вместо «No result provided».

8. **`requiredAlternatives` / `exactlyOneOf`** — второй контур валидации; наш
   `validate_tool_arguments` их не знает, а `edit` (5 режимов) и `lsp` (22 действия)
   без них живут на ручных проверках внутри `run()`.

9. **Confidence у них — фикция модели; отката и staleness нет.** Наш
   `rollback_proposal` — преимущество; к нему нужны **decay** и **авто-rollback по
   сигналу регрессии**.

10. **Два парсера taste (runtime vs lint) несовместимы** — урок: не парсить
    markdown-формат как источник истины. У нас JSON-структура; сохранить это.

11. **Правила выбора тула — в описание тула, а не в memory.** Их единственный тул с
    секцией «When NOT to use» (`todo_write`) — единственный без багов «не тот тул».
    Memory рендерится только когда релевантна запросу; описание — всегда.

## 10. ЧТО ИЗ TASTE СТОИТ ПЕРЕНЯТЬ в наш continual harness

У нас есть `refine.run()` с `memory / skill / prompt / subagent / factory` и
`HarnessEntry` (JSON). Ниже — только то, что **добавляет ценность поверх** уже
имеющегося, с указанием, куда именно в нашем коде.

### 10.1 Лестница приоритетов в промпте (сейчас отсутствует)

`crates/pa-core/src/refinement/ranking.rs:195 format_harness_state_for_prompt` —
добавить три строки **после** вводного абзаца:

```
Priority when they disagree: the user's current request > a continual-harness entry >
general best practice. An entry never overrides an explicit instruction in this turn;
treat that as the entry being updated, not as an exception.
```

Дословно у них (@2052400): «If a taste preference conflicts with the user's current
request, follow the current request (it may be updating the preference)» — плюс обратная
строка «Taste preferences override general best practices». Вместе они дают полную
лестницу; у нас нет ни одной из двух.

Почему это важно: у них это единственный работающий механизм разрешения конфликтов,
и он **бесплатный** (текст, не код). У нас его нет — а именно наш harness содержит
жёсткие инструкции («Call contract: … do not invent wrappers»), которые могут
конфликтовать с запросом пользователя.

**Тест:** `format_harness_state_for_prompt(...)` содержит подстроку «user's current
request >»; и при наличии memory-entry с текстом, противоречащим инструкции в
`<instructions>`, промпт всё равно содержит обе (мы не пытаемся их «помирить» — мы
объявляем приоритет).

### 10.2 Кодовый conflict-check при создании entry (у них — промпт)

`crates/pa-core/src/refinement/planner.rs:193 validate_edit` — сейчас проверяет только
id/slug/kind. Добавить: при `action: Create` посчитать пересечение
`harness_query_terms(content)` (уже есть, `ranking.rs:43`) с существующими entry того
же `kind`; при Jaccard ≥ 0.6 → `Err("duplicate/near-duplicate of {id}; use action: Update")`.

Почему: у них «prefer amending existing files over creating near-duplicates» — строка в
промпте learner'а (@1996187), и она **не работает**: issue #382 показывает, как
накапливаются противоречащие правила. Код надёжнее.

**Тест:** create с 80 % общих термов → `Err`; create с новыми термами → `Ok`;
update того же id при тех же термах → `Ok`.

### 10.3 Staleness: `last_confirmed_at` + исключение из дайджеста

Добавить в `HarnessEntry` (`crates/pa-core/src/refinement/mod.rs:51`) поле
`last_confirmed_at: Option<String>` и в `format_harness_state_for_prompt` — фильтр:
entry, не подтверждённый за `N` сессий, **не рендерится** (но остаётся в state и
виден через `harness.search`).

Почему: у них этой проблемы **нет решения** — мейнтейнер прямо говорит «это твоя
ответственность удалять» (#382), и это главная жалоба на taste. У нас есть откат
(`rollback_proposal`), но нет автоматического «протухания»; без него harness будет
накапливать устаревшее ровно так же.

**Тест:** entry с `last_confirmed_at` старше порога отсутствует в
`format_harness_state_for_prompt` и не влияет на `harness_digest_fingerprint`;
`harness.search` его находит.

### 10.4 Авто-rollback по сигналу регрессии

`rollback_proposal` (`planner.rs:505`) уже есть — добавить **триггер**: если после
применения refinement следующие K сессий дали `outcome: worse` (по нашим метрикам:
рост tool-call ошибок / повторов / отказов), применить `rollback_proposal`
автоматически и записать событие в `refinement_history.jsonl`.

Почему: это ровно то, чего у них нет («taste downgraded» — только если модель сама
написала меньшую цифру; удаление правила вообще не сигнализируется). У нас есть
механизм отката — ему не хватает триггера.

**Тест:** после `apply(create X)` и `record_outcome(worse) × K` →
`refinement_history` содержит `Rollback`-запись, state не содержит X.

### 10.5 Гард «entry не правится в обход gate» (у них — permission-промпт)

У них: запись в taste-файл **не** авто-аппрувится, отдельный промпт «Modify taste file
… — taste is normally managed by the learning system» (@1277073), плюс жёсткое правило
в промпте «NEVER EDIT OR WRITE TO TASTE FILES» (@2052400).

У нас: harness-состояние — обычный JSON-файл; главный агент может его отредактировать
через `edit`/`bash` в обход `refine.run()`, и тогда `refinement_history` разойдётся с
состоянием.

**ЧТО ДЕЛАТЬ:** добавить `harness/` в write-guard (как у них taste-директория) —
запись только через `refine.*`; при попытке прямого edit — либо блок, либо
предупреждение + запись в history «direct edit».
**Тест:** `edit` по пути `<agent_dir>/harness/state.json` → отказ с текстом «harness is
managed by refine.run()».

### 10.6 `harness lint`

Аналог их `taste lint` (@1421347), но для структуры:
- `title`/`content` непусты после trim;
- `path` внутри разрешённого корня (нет `..`);
- `reference`/`arguments` соответствуют контракту kind (у skill — `reference`+`arguments`,
  у memory — пусто);
- нет двух entry с одинаковым `(kind, normalized(title))`;
- `version >= 1`.

**Тест:** каждое нарушение даёт отдельный диагностический код; валидное состояние — 0
issues.

### 10.7 «When NOT to use» как первый класс — в описании, а не в memory

Это не про taste, но это **самый дешёвый** урок из всего исследования, и он напрямую
про наш continual harness: их единственный тул с настоящей дисциплиной описания
(`todo_write`, @1893373) — единственный, про который не было ни одного бага
«модель использовала не тот тул». Правило «When NOT to use» живёт в **описании тула**,
где оно применяется всегда, а не в memory, которая рендерится только когда релевантна
запросу.

**ЧТО ДЕЛАТЬ:** не писать в harness memory правила вида «не используй bash grep,
используй lsp». Вместо этого — в `description()` соответствующего тула, и в
статическом блоке «Working with these tools» (`system_prompt.rs:376`). Memory — для
проект-специфичных предпочтений, которые **нельзя** вывести из кода.

**Тест:** ни одна memory-entry не содержит имён наших тулов в императиве
(«use X», «don't use Y») — это ловится гардом на `harness lint` (см. 10.6) как
warning «tool-choice rule belongs in the tool description».

### 10.8 Компактная сигнатура в дайджесте (вместо JSON Schema)

Из их `buildToolSignature` @558714: `name{field: type, opt?: type}` с лимитом 160
символов на поле и 120 на первое предложение описания. Применить к нашим **skill**
entry (у них `reference`+`arguments` уже рендерятся компактно,
`ranking.rs` — `args={...} ref={...}`), чтобы дайджест не разрастался.

**Тест:** entry со 20 полями в `arguments` рендерится в ≤ 300 символов, содержит `?`
для необязательных.

---

## Приложение: воспроизводимость

Все проверки воспроизводятся скриптами в `/tmp/cc_verify/`:

| скрипт | что проверяет |
|---|---|
| `repair.cjs` | вырезанные `repairToolInput` + 12 правил + `resolveToolNameAlias` |
| `coerce.cjs` | `coerceToolInput` + `wrapBareStringRoot` + alias-таблица |
| `cmp.cjs` | `extractLearningWithConfidence` / `compareLearnings` |
| `run.cjs` | `runTasteTool` / `resolveTastePath` / `isValidTasteFilePath` |
| `reorg.cjs` | `parseCategories` / `categorySlug` / `reorganizeIfNeeded` |
| `lint.cjs` | `validateTasteFile` |
| `c1..c22.cjs` | кейсы (вывод в тексте выше); `c20` — alias + `block`-строка, `c21`/`c22` — двухпроходный sweep |

Запуск: `node /tmp/cc_verify/cN.cjs`.

Ограничение: функции вырезаны из минифицированного бандла **как есть**, с шимом
(`__name`, `readString2`, `errorResult`, `textResult`), и вызываются в изоляции от
рантайма. Это доказывает **логику** правил, но не доказывает, что полный пайплайн
(`execGuarded`) ведёт себя так же при всех комбинациях — для этого нужен живой прогон
CLI с реальной моделью (не делался: требует API-ключ и тратит кредиты).
