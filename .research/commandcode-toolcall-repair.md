# Command Code: механика починки tool call и роль Taste в цикле правок

Углубление отчётов `.research/taste-error-reduction.md` (admission gate) и `.research/toolcall-bestpractices.md`.
Фокус: цикл `tool-call → ошибка/правка → починка/перенаправление` и место taste в нём.
Версия продукта: command-code 1.84.0 (`/usr/lib/node_modules/command-code`, бандл `dist/cli.mjs`, 3.36 МБ, минифицирован — ниже `@N` = смещение символа в бандле, стабильно для этой сборки).

---

## 0. Главный вывод (по пунктам задачи)

1. **Починка tool call — это ЧЕТЫРЕ детерминированных слоя ДО исполнения + auto-continuation ПОСЛЕ обрыва.** Никакой LLM в цикле починки нет: провайдер-уровень (coerceToolInput), имя тула (resolveToolNameAlias, silent-redirect с переносом аргументов), схема-репэйр (13 именованных правил + таблица алиасов), и только если required-поля всё ещё битые — reject с консолидированной коррекцией (не stack trace). При успехе в вывод тула добавляются `<repair_note>` — «учебник канонической формы» для модели.
2. **Taste в этом цикле НЕ участвует как рантайм-фильтр.** Taste — фоновый learner на ОТДЕЛЬНОМ проходе (после завершения run'а), который пишет markdown-файл `taste.md`; файл целиком вставляется в системный промпт секцией `<taste>` и перечитывается ПРИ КАЖДОМ запросе к модели. То есть «пользователь правит за мной grep→find» превращается в правило taste → в следующем запросе модель ВИДИТ правило в промпте → меняет вызов сама. Перехвата/подмены вызова в рантайме taste не делает.
3. **Pre-call перехват есть, но не в taste:** (а) user-hooks `PreToolUse` (settings.json) — только block/context/stop, вход НЕ переписывают; (б) **mods `beforeToolCall` — могут ПЕРЕПИСАТЬ input, подсунуть синтетический результат или заблокировать вызов** — это полный аналог omp input-override.
4. **Цикл обучения:** конец run → `learning` mod → detached learner (собственная модель = модель сессии по умолчанию, BYOK→выкл) → инкрементально анализирует ТОЛЬКО новые genuine-user сообщения (старые — контекст) → tool-вызовы read/write/edit_taste_file → diff `compareLearnings` → сигналы `taste learned/upgraded/downgraded (65% → 80%)` → при >5 правилах в категории автореорганизация в `{category}/taste.md`. Admission gate из нашего раннего отчёта реализован слабее, чем мы предлагали: «gate» = системный промпт learner'а («Do NOT re-record… unless the NEW messages themselves contain fresh evidence») +confidence в самой строке. Это мягкий договор, не код.

---

## 1. Карта цикла починки tool call (код)

### 1.1 Слой 0 — провайдер-стрим: `coerceToolInput` (@953940)

Вызывается на каждый `tool-call` из стрима ДО попадания в историю (@959680, @969060). Чинит КОРЕНЬ аргументов, до схемы тула:

```
массив из 1 элемента       → развернуть (arr[0])
null                       → {}
строка-JSON ('{"a":1}')    → JSON.parse в объект
голая строка "..."         → wrapBareStringRoot: если в схеме ровно 1 required string-параметр —
                             завернуть как {param: text} (массив — {param:[text]})
```
Событие `tool_input_coerced` (toolName, rawType: array|null, recovered: bool, arrayLength) → телеметрия-спан `tool_input_coerced:<tool>` с `code: recovered?"recovered":"discarded"` (@1021686). Битый JSON с невосстанавливаемым корнем чиниться не пытается — discard.

### 1.2 Слой 1 — имя тула: `resolveToolNameAlias` (@817361), таблица `kC` (@2008594)

Silent-redirect ЦЕЛИКОМ на другой тул с переносом аргументов и дефолтами:

```
bash_output       → shell_output
task_output       → shell_output (defaults: wait:"exit" — если block!==false)
tool_search       → search_tools          (retired-имя)
read_multiple_files → read_file
bash / Bash       → shell_command  (gateAsCanonical — для permission-гейта считается каноничным)
```
В вывод добавляется repair-note: «the tool `bash` is now `shell_command`; this call ran as shell_command with your arguments carried over. Call `shell_command` directly next time.» Правило `renameAliasedTool` попадает в общий список repairs.

Несуществующее имя → плоская ошибка `Unknown tool: <name>` (@822203) — БЕЗ fuzzy-подсказки по именам тулов (fuzzy «Did you mean» есть у путей файлов, model id и config-ключей, но не у тулов). Имена тулов модели искать через deferred-механику `search_tools` (`select:<name>`).

### 1.3 Слой 2 — схема-репэйр: `repairToolInput` (@587715) + каталог `zf` (@1932672) + алиасы `Rf` (@1924195)

Каталог `zf` — упорядоченный список из 13 именованных правил (первое сработавшее выигрывает, sweep в 2 прохода: исходные ключи, затем появившиеся после репэйра; рекурсивно в объекты внутри массивов, с префиксом `key[i].`):

| # | правило | что делает |
|---|---|---|
| 1 | dropNullOrUndefinedField | выкинуть null/undefined-поля |
| 2 | dropEmptyObjectPlaceholder | выкинуть `{}`-заглушки |
| 3 | coerceStringToNumber | "42" → 42 |
| 4 | coerceStringToBoolean | "true" → true |
| 5 | parseJsonStringifiedArray | '["a","b"]' → массив |
| 6 | parseJsonStringifiedObject | '{"a":1}' → объект |
| 7 | wrapBareScalarAsArray | скаляр → [скаляр] |
| 8 | stripMarkdownLinkFromPath | `[x](path)` → path |
| 9 | stripOptionalMarkerFromKey | `pattern?` → `pattern` (нотация опциональности из доков тула) |
| 10 | renameAliasedField | алиасы → каноническое имя (таблица `Rf`, см. ниже) |
| 11 | rejectUnsupportedAction | `action:"kill"` в shell_output → выкинуть + подсказка «call kill_shell instead» |
| 12 | dropUnknownKey | выкинуть неизвестные поля |

Таблица `Rf` — десятки алиасов per-tool per-field, например:
- `read_file.file_path ← path, filePath, filepath, pathname, target_file, targetFile, file, absolute_path, absolutePath, fileAbsolutePath`
- `grep.pattern ← query, regex, search, q, expression, text`; `grep.path ← directory, dir, folder, searchPath, search_path`
- `edit_file.old_string ← oldValue, oldString, old, old_str, oldStr, from, old_value, oldText, …`
- `shell_command.command ← cmd, bash, shell, script, commandLine`; `…timeout ← timeout_ms, timeoutMs`

Правило 11 (`Wf` @~1927900) — единственное «перенаправление на другой тул» на уровне схемы: таблица `shell_output` → `noop:[read,get,output,poll]`, `elsewhere:{kill/stop/terminate→kill_shell, list/status→shell_tasks, start/run→shell_command}` — т.е. модель, путающая подкоманды, получает not «error», а «этот тул только читает; для kill вызови kill_shell».

### 1.4 Слой 3 — reject с коррекцией: `formatToolInputError` (@~588300)

Если ПОСЛЕ репэйра остаются нарушения required/exclusive-group/empty (`collectInputViolations` @584889) — тул НЕ исполняется, модель получает консолидированное сообщение:

```
Invalid input for tool "read_file". Please correct and retry:
  • file_path: required
Received fields: ...
Field-name corrections already recognized — apply them when you retry:
  • Renamed `path` to `file_path` ...
+ (deferred-тул) Call `search_tools` with `select:read_file` for this tool's full schema before retrying.
+ (script-first) Call `describeTool("read_file")` inside the script ...
```
(`schemaHint` @820547). Ретрай — забота модели (это обычный tool-result error), НЕ автоматический перезапрос.

### 1.5 После успеха: `<repair_note>` в выводе + телеметрия

`withRepairNotes` (@582556) / `applyRepairNotes` (@~588700): каждое сработавшее правило добавляется тегом `<repair_note>…</repair_note>` ПЕРЕД выводом тула — модель на каждом отрепэйренном вызове видит, какой канонической формы надо было придерживаться. `extractRepairNotes` обратно вырезает их при… (используется при повторной подаче в историю).

Событие `tool_input_repaired` (@637810, в `execGuarded` @637425): toolName, rulesFired (имена правил), hintCount, receivedKeys, repaired-флаг. Телеметрия: спан `tool_input_repaired:<tool>`, атрибуты `repair.outcome` (recovered/unrecovered), `repair.rule_fired`, `repair.received_keys`, `repair.received_root_type`, `repair.hint_count`, `gen_ai.request.model` (CHANGELOG 0.40.2 «stamp the active model on tool-input repair #2231» — репэйр-профили по моделям). Диаглог в файл при `CMD_LOG_REPAIRS=1`: `[cmd:repair] tool=… outcome=… rules=… hints=… keys=…` (@~2166500).

### 1.6 Обрыв генерации: continuation repair (mod `continuation`, @811008)

- stop `max_tokens` → до 3 авто-продолжений «Continue exactly where you left off — do not repeat content» (`length`);
- текст оборвался на «повисшей интенции» (endsWithDanglingIntent: заканчивается на `:`, тире, незаконченное перечисление) → до 2 продолжений (`intent`);
- пустой ответ → до 2 повторов «Please continue your response…» (`empty`);
- счётчики сбрасываются, если в ходе turn'а были tool-calls; refusal → notice и стоп. Событие `continuation_recovery` → спан `continuation_recovery:<kind>`. (CHANGELOG 0.36.0 «agent continuation repair harness strong long term looping».)

### 1.7 Порядок всего конвейера одного вызова

```
tool-call из стрима
 → coerceToolInput (корень аргументов)                     [слой 0]
 → mods[].beforeToolCall  — rewrite input / block / synthetic result / context / terminate  (см. §4)
 → permissions (mode; для записи в taste-файл — спец-промпт, см. §3.6)
 → toolRunner.execute:
     resolveToolNameAlias (имя тула, silent-redirect)      [слой 1]
     repairToolInput (13 правил + алиасы Rf)               [слой 2]
     violations? → reject: correction + hints + schema-steer [слой 3, тул не исполняется]
     run(исправленный input)
     truncate (25K токенов; spill-файл) + applyRepairNotes(<repair_note>…)
 → afterToolCall (user-hooks advisory; mods: rewrite result / isError / terminate)
```

История фич (CHANGELOG): 0.24.0 «tool auto repair and healing, lenient LLM tool-input healing»; 0.28.0 «36K+ repair variations fixed across 29 models» (репэйр-матрица по моделям); 0.30.0 «repair v3: read_file/edit_file/glob alias repairs (#1869)»; 0.40.2 stamp модели в repair-телеметрии.

---

## 2. Живые данные taste на этой машине (схема, не контент)

Глобального `~/.commandcode/taste/` нет. Есть два проектных: `/root/daily-archive/.commandcode/taste/taste.md` (925 B) и `/root/worktrees/e1-critic/.commandcode/taste/taste.md` (байт-в-байт тот же — worktree-копия). Категорийных файлов нет (категорий >5 правил не набралось — см. reorganize).

Формат файла (единственный формат записи; «записей» как объектов нет — это просто markdown):

```markdown
# Taste (Continuously Learned by [CommandCode][cmd])

[cmd]: https://commandcode.ai/

# <category>                       ← заголовок категории (slug при reorganize)
- <правило, одна строка>. Confidence: 0.65
```

Живая эволюция за 45 минут одной сессии (file-history v1→v6, бэкапы чекпойнтятся как обычные файлы):
- v1 (15:11): 2 правила. «prioritize concrete task first… Confidence: 0.75»
- v2: та же строка → **Confidence: 0.80** (повторное подтверждение подняло скоринг — `taste upgraded`)
- v3: +«Refer to self as CommandCode… 0.70» → v4: → **0.80**
- v5: +«Do not guess or fabricate… 0.85»
- v6: +«delegate to subagents… 0.65»
Итого 5 правил, 2 upgrade'а, 3 добавления. Confidence-значения ставит learner-модель (никакой формулы в коде нет — только диффуз строк).

`history.jsonl`: `{"p":"/model","t":<epoch_ms>}` — история слэш-команд (69 строк). `trusted-hooks.json`: `{}` — доверенных хуков нет.

---

## 3. Полный цикл taste: триггер → learner → запись → применение

### 3.1 Триггеры (два пути)

**(a) Автоматический.** Внутренний mod `{id:"learning", onRunEnd}` (@813443, подключение @1059445): по окончании КАЖДОГО run'а, если taste включён и learner не запущен — `startTasteLearnerDetached` (Wt @1058998). Learner сам no-op'ится, если нет НОВЫХ genuine-user сообщений (`isGenuineUserMessage`: role=user, не automated/meta/summary/tool-result). На /clear фоновое обучение стопится (CHANGELOG).

**(b) Явный — тул `taste`** (@591469, deferred, searchHint "style preferences taste file"): «Use this whenever the user states a preference or asks you to remember something», input `{instruction}`. Возвращает мгновенно: «On it — learning your preference in the background. I'll save it and the TASTE note will appear when it's done. Carry on.» — и стартует detached learner с focusInstruction, дописанным в конец сообщений как user-text. Выкл → «Taste learning is off… Turn taste on with /taste…»; BYOK → «unavailable on BYOK».

### 3.2 Learner (`createLearner` @787499)

- **Модель:** feature-model `tasteLearning`; default = ТЕКУЩАЯ модель сессии (`defaultFeatureModel` @124740: `if ("tasteLearning"===key) return config.getModel()` — ridesMainModel), переопределяется через /config (feature-model:tasteLearning). Отдельного «taste-1» инференса в бандле не видно — это промпт-агент над тем же API (`modelClient oe("learning")`).
- **Инкрементальность:** помнит индекс последнего обработанного сообщения; в промпт идут: текущая структура taste-файлов + РАНЕЕ разобранные сообщения (только контекст, «do NOT learn from it again») + ТОЛЬКО НОВЫЕ (learn from these). Окно dk=20 сообщений, до ck=20 tool-раундов learner'а.
- **Фильтрация входа:** `stripInjectedMetaForLearning` (сбрасывает meta.injected), `stripReasoningPartsForLearning` (выбрасывает thinking-блоки), `isLearnableMessage` (выбрасывает automated/meta/summary и user-сообщения с tool_result). Ассистентские tool_use (имя+input) — ВИДНЫ learner'у, сырые результаты тулов — нет.
- **Системный промпт learner'а (`uk` @1996183)** — дословно ключевые строки: «record DURABLE, generalizable preferences… not one-off task details»; «Do NOT re-record a preference that already exists… do NOT raise or lower an existing learning's confidence unless the NEW messages themselves contain fresh evidence for it»; формат «`- <rule>. Confidence: 0.9`»; «When the new messages reveal nothing durable, make no tool calls and reply "no changes"». Это и есть «admission gate» на практике: он целиком внутри промпта вторичной модели.
- **Тулзы learner'а (`mk` @1997596):** `read_taste_file` / `write_taste_file` / `edit_taste_file`, пути ЖЁСТКО ограничены `taste.md` или `{category}/taste.md` (`isValidTasteFilePath`, resolveTastePath блокирует `..`). Никаких других действий у learner'а нет.
- **Запись (`runTasteTool` @786501):** edit — replace первого вхождения old_text; write — целиком. После каждой записи `emitLearningSignals` (@785868): дифф `compareLearnings` (@782208) по строкам `- … Confidence: X` → notice-события `taste learned: <rule> (65%) → taste.md`, `taste upgraded: <rule> (70% → 80%)`, `taste downgraded: …` — они же рендерятся как TASTE-блоки в ленте (Ctrl+T — learning feed; в session-jsonl не пишутся).
- **Реорганизация (`reorganizeIfNeeded` @785213):** категория в корневом taste.md с >5 правилами выносится в `<slug>/taste.md`, а на её место — «See [slug/taste.md](…)» + notice «taste: moved …».

### 3.3 Применение — ТОЛЬКО промпт, без рантайм-гейта

`getTasteContent` (@1017138): глобальный `~/.commandcode/taste/taste.md` + проектный `.commandcode/taste/taste.md`, склейка. `renderTasteSection2` (@913344) оборачивает в секцию системного промпта:

```
<taste>
Below is the complete content of the .commandcode/taste/taste.md file. …
If you see references like "See [category/taste.md]", you MUST read that file using read_file …
--- Content of .commandcode/taste/taste.md ---
…текст файла…
--- End … ---
</taste>
```
(пусто → «No preferences learned yet… Preferences will be learned automatically as you work.»)

Позиция в промпте (`createSystemPromptBuilder` @923965): base → `<instructions>`(memory) → workspace → **`<taste>`** → skills → explore-agent → scratchpad → env. Сборка — **на каждый запрос модели** (@647878: per-round в agent loop, перед `callModelWithRetry`), т.е. изменение taste.md подхватывается уже СЛЕДУЮЩИМ вызовом в той же сессии. Кэша нет, файл перечитывается с диска.

### 3.4 Живой кейс из сессии (daily-archive, 2026-07-19)

- 15:10:16 юзер: «подключи mcp gitnexus». Агент ищет ГЛОБАЛЬНЫЕ конфиги (~/.claude.json и т.п.).
- 15:11:20 юзер ПРАВИТ: «подключи mcp gitnexus как проектный mcp через .mcp.json».
- 15:11:34 (через 14 сек, БЕЗ tool-call в транскрипте — фоновый learner) — создаётся taste.md v1: «Use project-level MCP configuration via .mcp.json rather than global MCP config files. Confidence: 0.65».
- 15:11:51 в reasoning следующего вызова: «According to the taste, I should use project-level MCP configuration via .mcp.json» → write_file .mcp.json. **Полный оборот правка→learning→применение = 31 секунда, в той же сессии.**
- Дальше по версиям: правило подтверждается повторно (0.65 не трогали), растут другие.

Это эмпирически подтверждает схему «correction → candidate → (мягкий admission) → запись → перечитка промпта → смена поведения», без какого-либо перехвата вызова.

### 3.5 Защита от рук главного агента

Permission-слой (@1277080): запись в файл внутри taste-директории не авто-аппрувится — отдельный промпт «Modify taste file … — taste is normally managed by the learning system». Плюс `isPathInTasteDirectory` (@200582). Т.е. taste-файлы «принадлежат» learner'у; главный агент их трогает только через явное разрешение юзера.

### 3.6 Управление

`tasteLearning: bool` в user-config (project|user scope, 0.38.5+; legacy-load @115333, «isTasteLearningEnabled: !==false» @117787); BYOK → всегда выкл; `/taste`, `cmd taste …` (push/pull/list/lint/open), `/learn-taste` (импорт из сессий Claude Code/Cursor — createSessionImporter + getTasteContent @1017138…). Скрытие taste-нотисов — /config (1.68.0).

---

## 4. Pre-call перехват: user-hooks vs mods (аналог omp input-override)

**User-hooks (settings.json, PreToolUse/PostToolUse/Stop/SessionStart; `createUserHooksMod` @811899).** Внешний процесс, JSON на stdin. PreToolUse может: `block` (deny с сообщением), `stop` (halt сессии), `systemMessage`/`addedContext` (контекст в следующий ход). **Input переписывать НЕ может** — валидные поля жёстко фильтруются (`getValidFieldsForEvent`), неожиданные поля игнорируются с warning. На этой машине trusted-hooks.json пуст.

**Mods (`beforeToolCall` в ModApi; диспетчер `executeOne` @638363, композер @253139).** In-process TypeScript-плагин. Возвращает ЛЮБОЕ из:
- `{input: <new>}` — **переписать аргументы вызова** (цепочка модов: output предыдущего = input следующего; исполнение идёт с итоговым input);
- `{result: …}` — **подменить результат, тул не исполняется вовсе**;
- `{block: true, additionalContext}` — заблокировать (модель получает error-result с текстом хука);
- `{additionalContext}` — добавить контекст; `{terminate}` — стоп сессии; `{modState}` — своё состояние.
Аналогично afterToolCall переписывает результат/content/isError. Это и есть полный omp-style input-override, только событие — pre-EXECUTION (после генерации вызова моделью), плюс transformInput для typed-input пользователя и transformTextDelta/transformSystemPrompt для стрима.

Порядок: **mods.beforeToolCall → permissions → alias → repair → run** — мод видит сырой вызов модели ДО deterministic-repair.

---

## 5. Отличие от omp-подхода (из нашего debugger-исследования)

| ось | omp input-override | Command Code |
|---|---|---|
| Кто переписывает | внешний дебаггер/оркестратор на входе модели (input-override до генерации) | (а) mods ПОСЛЕ генерации, до исполнения; (б) repair-слой — детерминированный, не модель |
| Область | перехват пользовательского ввода/промпта | только tool-call аргументы/результаты + typed input + стрим текста |
| Обучение | внешнее (у нас) | repair_note учит модель канонической форме в том же контексте; taste учит ПРОДУКТ (файл правил), не конкретный вызов |
| Рантайм-гейт по выученным правилам | был возможен | отсутствует: taste влияет только через следующий системный промпт |
| Наблюдаемость | трейс дебаггера | `tool_input_repaired/coerced`-события, спаны, CMD_LOG_REPAIRS; repair-правила попадают в repairs-массив результата |

Ключевая разница философии: omp — «перехвати и почини до/в момент», Command Code — «почини детерминированно и НАУЧИ (repair_note + taste-файл), чтобы следующий вызов родился правильным».

---

## 6. Применимость к нашему форку (pa-jev arbiter + trace-инварианты)

1. **Repair-каталог как дешёвый первый слой.** 13 правил + алиас-таблица — это чистые функции над (schema, input), ноль токенов, ноль латентности. Для нашего `bash/edit/…` в pa-core это прямой импорт: alias-таблица (`path→file_path`, `cmd→command`, `oldValue→old_string`), coercion, JSON-stringified-parse, drop-null. Приоритет: правила, которые у нас чаще всего ломают вызов (алиасы имён полей edit/bash).
2. **`<repair_note>` — верифицируемое обучение канонической форме.** Превращает каждый отрепэйренный вызов в микpо-урок в контексте. Дёшево (десятки токенов) и уже оправдано их «36K+ variations across 29 models».
3. **'grep-вместо-graph.search' — это поведение-кандидат в taste, НО не через рантайм-гейт, а через запись + промпт.** Схема Command Code подсказывает конкретную реализацию для pa-jev: trace-инвариант «юзер/арбитр переписал вызов graph.search→grep или наоборот» = signal; arbiter (наш аналог learner'а) генерирует candidate-строку `- Prefer graph.search over grep for symbol lookup in this repo. Confidence: X`; admission (наш gate, СИЛЬНЕЕ ихнего — у нас код, не только промпт); запись; инъекция в динамический хвост промпта при следующей сборке (у нас это динамический tail системного промпта — перечитка per request уже так устроена).
4. **Их слабое место = наш шанс:** admission у них — строки в промпте вторичной модели; confidence не калиброван ничем, кроме фантазии learner-модели (живые данные: 0.65→0.80 без видимой формулы). Наш ранний отчёт (§3.2, §6.2) уже предлагает кодированный gate с re-score/conflict-check — стоит добавить из этого исследования только эмпирику формата: одна строка = одно правило + confidence в конце строки, категории как заголовки, порог >5 правил на категорию для реорганизации.
5. **Защита владельца.** Их permission-guard «taste is normally managed by the learning system» — правильный паттерн для нас: refinement-записи (refine.*) не должны молча редактироваться главным агентом в обход gate.
6. **mods beforeToolClose-аналог не нужен** — у нас omp-слой (внешний) покрывает rewrite-кейсы; но их порядок «мод видит сырой вызов ДО детерминированного репэйра» полезен как принцип: semantic-арбитр должен получать ДО-репэйр вызов, иначе он не увидит саму ошибку модели (и не сможет сделать taste-кандидат из неё).
7. **Телеметрия репэйра как датасет.** `repair.rule_fired × model × received_keys` — готовая схема события для pa-telemetry: adoption-событие на каждый repair (правила и ключи, не значения) — совпадает с нашим правилом «primitives only».

---

## 7. Маркеры для быстрой навигации по бандлу (v1.84.0)

| что | маркер | @offset |
|---|---|---|
| coercion корня аргументов | `function coerceToolInput` | 953940 |
| редирект имён тулов | `function resolveToolNameAlias` / `kC={bash_output:` | 817361 / 2008594 |
| каталог репэйров | `zf=[{name:"dropNullOrUndefinedField"` | 1932672 |
| алиасы полей | `Rf={read_file:{file_path:[` | 1924195 |
| action→tool перенаправление | `Wf={shell_output:{noop:` | ~1927900 |
| репэйр-пайплайн | `function repairToolInput` | 587715 |
| violations/коррекция | `function formatToolInputError` / `collectInputViolations` | ~588300 / 584889 |
| repair_note | `function withRepairNotes` / `applyRepairNotes` | 582556 / ~588700 |
| executor | `function createToolRunner` (execute) | ~821900 (exec @822203) |
| событие репэйра | `tool_input_repaired` | 637810 |
| телеметрия репэйра | `buildRepairTelemetryEvent` / `CMD_LOG_REPAIRS` | ~2166500 |
| continuation repair | `function createContinuationProvider` / `Zk=` | 811008 / 2005244 |
| тул taste | `function createTasteTool` | 591469 |
| learner | `function createLearner` | 787499 |
| sysprompt learner'а | `uk='You are the taste-learning agent` | 1996183 |
| тулзы learner'а | `mk=[{name:"read_taste_file"` | 1997596 |
| запись/diff | `function runTasteTool` / `emitLearningSignals` / `compareLearnings` | 786501 / 785868 / 782208 |
| реорганизация | `function reorganizeIfNeeded` | 785213 |
| инъекция | `function renderTasteSection2` / `getTasteContent` | 913344 / 1017138 |
| сборка промпта per-round | `createSystemPromptBuilder` + вызов | 923965 / 647878 |
| триггер learning-mod | `createLearningMod` / `startTasteLearnerDetached` | 813443 / 1058998 |
| модель learner'а = сессия | `function defaultFeatureModel` | 124740 |
| guard taste-файлов | `Modify taste file` / `isPathInTasteDirectory` | 1277080 / 200582 |
| user-hooks mod | `function createUserHooksMod` | 811899 |
| mod beforeToolCall rewrite | `function executeOne` | 638363 |

Живые данные: `/root/daily-archive/.commandcode/taste/taste.md` (+версии v1–v6 в `/root/.commandcode/file-history/d9baebdb-…/9d07363681a460d2-47@v*`); сессия-кейс `/root/.commandcode/projects/root-daily-archive/d9baebdb-…jsonl` (строки 1–16: правка юзера → taste-правило → применение за 31 сек).
