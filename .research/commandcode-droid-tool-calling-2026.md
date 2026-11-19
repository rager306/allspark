# Коррекция tool calling по семейству моделей: CommandCode, Droid и другие

**Статус:** исследование. Второй срез к `omp-tool-steering-2026.md`.
**Источник CommandCode:** обновлён локально 1.84.0 → **1.91.1**, разобраны
`dist/cli.mjs` и 76 файлов `dist/bundled/`.
**Вопрос:** как корректируют поведение агента *под конкретное семейство моделей*,
и что из этого применимо к нашему RLM REPL.

## 1. Что именно делает CommandCode (прямые находки)

### 1.1 `behaviorProfile` — профиль поведения per-model

Таблица профилей (`dist/cli.mjs`) — не про промпт, а про **провод**:

```js
$n = new Map([
  [default, {supportsDeferredTools: true,  usesInterleavedThinking: false,
             bindsThinkingToPrefix: false, acceptsForcedToolChoice: true}],
  ["no-deferred-tools-v1",         {supportsDeferredTools: false, ...}],
  ["private-reasoning-v1",         {usesInterleavedThinking: true,  ...}],
  ["anthropic-interleaved-v1",     {supportsDeferredTools: true, usesInterleavedThinking: true, ...}],
  ["anthropic-prefix-bound-v1",    {bindsThinkingToPrefix: true, acceptsForcedToolChoice: false, ...}],
  ["anthropic-prefix-bound-forced-v1", {bindsThinkingToPrefix: true, acceptsForcedToolChoice: true, ...}],
])
```

Четыре флага, и каждый лечит конкретный известный дефект семейства:
`supportsDeferredTools` (можно ли грузить схемы по требованию),
`usesInterleavedThinking`, `bindsThinkingToPrefix` (thinking жёстко привязан к
префиксу — иначе провайдер отвергает), `acceptsForcedToolChoice` (принимает ли
`tool_choice: required`).

Вывод: **коррекция по семейству у них — это флаги возможностей, а не текст
промпта.** Модель не «уговаривают» — ей не предлагают то, что её провод не
примет.

### 1.2 `supportsDeferredTools` — размер набора как per-model свойство

`executeDeferredToolSearch` возвращает при отказе:

> `No deferred tool matched .... The names available to load are listed under
> "Additional tools" in your system prompt.`

и при успехе:

> `Loaded N tool schema(s). Call them directly by name from now on, using exactly
> these parameters.`

То есть приём «мало инструментов в промпте + подгрузка схем» включается **только**
для моделей, чей профиль это поддерживает. Для остальных набор остаётся полным.
Это прямо отвечает на наш #43: там, где набор большой, его уменьшают, а где
модель не умеет — не притворяются.

### 1.3 Repair-слой: чинят вход, а не выбор

Документ `reference/tools.md` фиксирует пайплайн до исполнения:

> **Input repair.** A schema-driven repair layer heals malformed input before
> execution: JSON-stringified arrays and objects are parsed, bare scalars are
> wrapped, string numbers and booleans are coerced, markdown-wrapped paths are
> stripped, null placeholder fields are dropped, and **dozens of parameter aliases
> are renamed to the canonical name** (`path` → `file_path`, `query` → `pattern`,
> `oldValue` → `old_string`, `cmd` → `command`, …). Drifting models heal instead
> of erroring.

> **Repair notes.** Any repairs that fired are prepended as `<repair_note>` tags
> so the model learns the canonical shape — and each repair is emitted as a
> telemetry event (rule names and keys only, never your values).

Механизм в коде: `parseLearning`-подобный разбор и тег
`<repair_note>${…}</repair_note>`, извлекаемый регуляркой
`/<repair_note>([\s\S]*?)<\/repair_note>$/`.

**Ключевое отличие от подсказки о неверном инструменте:** здесь чинится
*аргумент уже выбранного* инструмента, и заметка приходит **вместе с
результатом**. Это ровно то, чего наш арбитр не делает для Python-привычки.

### 1.4 Правило «инструмент вместо shell-аналога» — в описании инструмента

Не в промпте и не после вызова, а внутри `description` инструмента исполнения:

> Use the dedicated tools for ordinary file work: glob for file search (not
> recursive `Get-ChildItem`), grep for content search (not `Select-String`),
> `read_file` (not `Get-Content`), `edit_file`/`write_file` (not `Set-Content` or
> `Out-File`).

То есть отрицательные примеры («не это») стоят там, где модель читает **до**
вызова. Наш арбитр говорит **после**. Это разные точки воздействия.

### 1.5 Отставленные инструменты помечаются, а не удаляются

`createRetiredToolSearchTool`:

> `Retired: use ${…} instead.`

Схема остаётся видимой с указанием замены — модель, помнящая старое имя,
получает редирект в описании.

### 1.6 Taste: обученные правила с confidence

Файл `taste.md` (реальный пример с этой машины):

```markdown
# Taste (Continuously Learned by [CommandCode][cmd])

# mcp
- Use project-level MCP configuration via .mcp.json rather than global MCP config files. Confidence: 0.65

# workflow
- When user makes a concrete actionable request alongside an open-ended one, prioritize the concrete task first. Confidence: 0.80

# communication
- Do not guess or fabricate answers when uncertain; research first using available tools, and if evidence isn't found, honestly say "I don't know". Confidence: 0.85
```

Формат разбирается `parseLearning`:

```js
const m = line.match(/^-?\s*(.+?)[\.:,]?\s*confidence:\s*(\d*\.?\d+)/i);
// confidence вне [0,1] → правило отбрасывается
```

Категории нормализуются в slug (`mcp`, `cli`, `workflow`, `testing`, `style`,
`architecture`), путь — `…/taste/<category>.md`, сборка — `formatPackage`.

**Taste — это про стиль и предпочтения, не про выбор инструмента.** В изученном
примере ни одного правила вида «используй X вместо Y». Но **механика** (правило +
confidence + отбрасывание сомнительных) применима и к нашим подсказкам.

## 2. Что делает Droid (и почему это важно для сравнения)

Droid (Factory) в публичных материалах делает акцент на другом: **специализация
поведения по роли модели**, а не по её семейству — разные агенты (droid,
`reviewer`, `implementer`), разные наборы инструментов и разные системные
промпты, плюс «spec mode» (план вместо действий). Проверяемой механики уровня
`behaviorProfile` в открытом виде нет: это продуктовая фича, а не документ.

Практический вывод: **два разных рычага** — «профиль возможностей провода»
(CommandCode) и «профиль роли агента» (Droid). Наш репозиторий уже имеет второй:
`[tools]` профили и `tool_profile` (`crates/pa-core/src/tool_profile/`).

## 3. Что это значит для нашего RLM REPL

| практика | у нас | вердикт |
|---|---|---|
| профиль возможностей per-model (флаги) | нет; `pa-ai` знает `Model`, но не «что этот провод не примет» | **стоит портировать идею**, но как флаги в каталоге моделей, а не текст |
| repair аргументов + `<repair_note>` | **частично есть**: `pa-agent/src/validation.rs` делает `coerce` (строковые числа/булевы → типы) и отдаёт ошибку в TS-формате. **Нет** двух вещей: таблицы алиасов (`path`→`file_path`) и заметки, которая учит канонической форме | **высокая ценность**: чинит причину, а не следствие. Разрыв узкий и точный — алиасы + заметка |
| уменьшение набора при `supportsDeferredTools` | нет; MCP уже отложен через `mcp.list_tools`, но skills inventory — нет | совпадает с выводами Anthropic (8.7k против 77k токенов) |
| «инструмент вместо shell» **в описании** | нет; у нас это в маршрутной таблице промпта | **дёшево и безопасно**: перенести отрицательные примеры в описания |
| retired-инструменты с редиректом | нет | полезно при переименованиях (`read_multiple_files` → `read_file` у них) |
| арбитр после вызова | **есть** (`tool_arbiter.rs`, advisory) | оставить: замер дал 84% без эффекта |
| `scriptFirst`-коррекция способа (read-only только из скрипта) | нет | **самое применимое** к нашему случаю: 96% вызовов уже идут через `python_repl`, и подсказка «вызови `graph.search` внутри ячейки» меняет способ, а не инструмент |
| taste (правила + confidence) | нет; у нас `.commandcode/taste` **не про это дерево** | идея применима к подсказкам, но не к выбору инструмента |

### Главный вывод для #43

Наш провал (4 `python_repl`, 0 `graph`) — это **не** «модель не послушала
подсказку». Это разрыв покрытия: арбитр видит только `bash()` внутри ячейки.
CommandCode в этой ситуации действует иначе — он **не даёт модели выбрать
неправильный уровень**: инструмент чтения файлов и инструмент исполнения
разделены, и отрицательные примеры стоят в описании.

## 4. Что мерить дальше (и чем)

1. **Repair-слой**: доля вызовов с исправленным аргументом. Метрика берётся из
   наших же логов: сколько tool-результатов содержат ошибку схемы и сколько —
   исправление.
2. **Набор инструментов против точности выбора**: `run_suite.py` c `must` по
   инструменту (ST-корпус) уже умеет это мерить; нужен A/B по размеру набора.
3. **Профиль провода**: сколько раз провайдер отверг наши параметры
   (`tool_choice`, thinking). У нас это видно в `provider_errors`.

## 5. Чего в этом исследовании нет

- **Droid проверен только по публичным материалам**, не по коду: бинаря и
  исходников на этой машине нет. Поэтому его раздел — гипотеза уровня продукта,
  а не измерение.
- Второй срез уже существующего исследования `commandcode-toolcall-repair.md`:
  тот документ описывает телеметрию репэйра; этот — сравнивает практики
  коррекции и их применимость к RLM REPL. Дублирования нет, но читать их стоит
  вместе.
- **CommandCode — минифицированный бандл** (4.4 MB `cli.mjs`): цитаты точные,
  но структура восстановлена по строкам, а не по исходникам.
- **Нет A/B** ни для одного пункта: все рекомендации — по аналогии и по их
  документированным замерам, не по нашим.

## 6. Главная находка: `scriptFirst` / Codemode — коррекция *способа*, а не инструмента

Это отвечает прямо на наш #43 и на вопрос «как поправлять выбор».

CommandCode делит инструменты на два класса и **не даёт** модели выбирать
неправильный:

```js
scriptFirst: ({model}) => {
  const scope = getCodemodeSettingSync();
  const codemodeOffered = supportsDeferredTools({model}) && modHost.isToolEnabled(…);
  return tool => isScriptFirstTool({tool, scope, codemodeOffered});
}

function isScriptFirstTool({tool, scope, codemodeOffered}) {
  return codemodeOffered && scope === "tools"
      && tool.schema.name !== <code-execution tool>
      && isInScope({name: tool.schema.name, scope, readOnly: tool.readOnly === true});
}
```

Читается так: **только read-only-инструменты** становятся «script-first» —
их предлагается вызывать из скрипта. Инструменты, которые правят, пишут или
запускают команды, **никогда** не вызываются из скрипта:

> Tools that edit, write, or run commands are never callable from a script; call
> those directly instead. Other names reject with an Error.

А модель, вызвавшая script-first инструмент напрямую, получает вместо ошибки
**коррекцию способа**:

```js
function scriptFirstSteer({toolName}) {
  return `"${toolName}" is a script-first tool this session. Next time call it
          inside a codemode script (\`await tools.${toolName}(args)\`), batch
          related calls in the same script, and return only the values you need.`
}
```

Три свойства, которых у нашего арбитра нет:

1. **Включается по модели**: `supportsDeferredTools({model})` — то же, что в
   §1.1. Для семейства без codemode коррекция не предлагается вообще.
2. **Корректирует способ, а не имя**: не «возьми другой инструмент», а «возьми
   этот же, но внутри скрипта, и верни только нужные значения».
3. **Считает выгоду**: обоснование в самом тексте — «a script that returns a
   whole file or a whole result list has saved nothing.» Это критерий, по
   которому видно, окупилась ли коррекция.

### Почему это отвечает на наш случай

Наш провал — 4 вызова `python_repl` вместо `graph` на вопросе о цепочке вызовов.
У CommandCode аналог нашего `python_repl` — это тоже «script-first» среда: туда
предлагается складывать батч read-only-вызовов. Но они **не подсказывают «не
используй python»**, они подсказывают **«используй его правильно»** — и только
когда модель реально вызвала read-инструмент напрямую.

В нашей архитектуре это отображение один-в-один: `graph`, `lsp`, `edit` — это
read-mostly инструменты; их естественный «script-first» режим — вызов
**внутри `python_repl`**, где модель уже работает 96% времени. Подсказка
«вызови `graph.search` напрямую, а не собирай то же из `bash`+`python`» —
это ровно `scriptFirstSteer`, только с другим именем.

### Ограничение, которое надо сохранить

Их правило «никогда из скрипта» относится к инструментам, которые *меняют*
состояние. Наш арбитр сформулирован для `bash`-команд и уже не блокирует —
значит для нас правило такое: **подсказка только для read-only инструментов**,
иначе она станет советом «делай правки в скрипте», что противоречит `edit`
контракту (нужен read-then-edit в одном ходе).
