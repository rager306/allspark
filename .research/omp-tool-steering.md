# Стеринг инструментов в oh-my-pi: есть ли перехват вызова с редиректом?

Источник: `/tmp/omp-src` (oh-my-pi / `omp`, workspace version `18.10.4`).
Режим: только чтение. Все ссылки `файл:строка` — на код omp.

---

## 0. Ответ в трёх строках

1. **Механики «подмени вызов на другой тул» в omp НЕТ.** Ни в ядре, ни в
   coding-agent. Хук `beforeToolCall` умеет ровно три вещи: **блок**,
   **подмену аргументов** того же тула и **пассивный контекст** — но не смену
   имени тула (`types.ts:903-930`).
2. **Что есть вместо:** (а) **блок с текстом-подсказкой** (bash-interceptor —
   единственный настоящий «редирект», но по факту это `throw`, а не вызов
   другого тула); (б) **алиас-резолвинг имени** — единственное место, где omp
   реально переписывает `toolCall.name` (`agent-loop.ts:3030`), но только на
   каноническое имя уже выбранного/смонтированного тула, не на «другой» тул по
   смыслу; (в) **промпт-стеринг** — negative-инструкции в системном промпте и
   условные вставки в описания тулов; (г) **динамическая видимость** через
   `loadMode`/`xd://`/plan-mode/`find.enabled`.
3. **grep→find стеринга на уровне вызова НЕТ.** Есть только текст в описаниях:
   `grep.md:4` («Behavior/unknown symbol → `find`») и `system-prompt.md:114-116`
   («Unknown behavior/location: descriptive `find` FIRST; NEVER guess
   `grep`/`glob` targets»). Никакого перехвата `grep`-вызова и подмены на `find`.

---

## 1. Перехват вызова до исполнения: что реально умеет хук

### 1.1. Контракт `beforeToolCall` (ядро)

`packages/agent/src/types.ts:590` — объявление хука; `types.ts:903-930` — результат:

```ts
export interface BeforeToolCallResult {
    block?: boolean;       // запретить исполнение
    reason?: string;       // текст ошибки модели
    args?: Record<string, unknown>;   // ПОДМЕНА АРГУМЕНТОВ того же тула
    additionalContext?: string;       // пассивный контекст к следующему запросу
}
```

**Поля для смены имени тула нет.** Комментарий `types.ts:576-588` прямо
описывает семантику: «Return `{ args }` to replace the arguments the call runs
with. The replacement is revalidated against the tool schema and written back to
the tool-call block». Контекст хука (`types.ts:937-951`) даёт `tool` (уже
разрешённый) и `args`, но не позволяет вернуть другой `tool`.

Реализация в цикле — `packages/agent/src/agent-loop.ts:3077-3111`
(`prepareToolCallDispatch`): хук вызывается уже ПОСЛЕ `resolveToolForCall`,
поэтому `tool` зафиксирован; обрабатываются только `block`, `additionalContext`
и `args` (с ревалидацией и записью обратно в `toolCall.arguments`).

### 1.2. Extension-хук `tool_call` — то же самое

`packages/coding-agent/src/extensibility/shared-events.ts:325-355`
(`ToolCallEventResult`): `block`, `reason`, `input`, `additionalContext`. Поле
`input` — это **аргументы**, не имя: «Replacement input the tool executes with…
This is the raw execution input passed to the tool's `execute`». Событие
`ToolCallEvent` (`extensions/types.ts:1288-1330`) вообще типизировано по
`toolName: "bash" | "read" | ...` как литерал — сменить его из хендлера нельзя.

Проводка: `session/agent-session.ts:4934-4983` (`#beforeToolCall`) → возвращает
`{ args, additionalContext }` или `{ block: true, reason }`. Для путей, которые
цикл не видит (nested `write xd://`, Cursor direct), та же логика в
`extensibility/extensions/wrapper.ts:275-305`, где `effectiveParams =
callResult.input` — опять только аргументы.

### 1.3. Единственное место, где omp ПЕРЕПИСЫВАЕТ имя тула

`packages/agent/src/agent-loop.ts:3024-3031`:

```ts
const tool = resolveToolForCall(context.tools, toolCall, resolveFallbackTool);
if (tool && toolCall.name !== tool.name && toolCall.name !== tool.customWireName) {
    toolCall.name = tool.name;   // канонизация имени
}
```

`resolveToolForCall` (`agent-loop.ts:2811-2829`) ищет по точному имени, затем по
`customWireName`, затем зовёт `resolveFallbackTool`. В coding-agent это
`resolveDeviceTool` (`sdk.ts:3659-3685`): снимает `xd://`-префикс, ищет
смонтированный device, затем пробует MCP-алиас (`resolveMCPToolAlias`,
`mcp/tool-bridge.ts:571-584`, требует УНИКАЛЬНОГО совпадения).

**Это редирект имени, но не редирект смысла**: `xd://recall` → тул `recall`,
`mcp__foo__bar__read` → канонический MCP-тул. Модель, вызвавшая `grep`, не
попадёт в `find` — такого маппинга в коде нет.

### 1.4. Единственный настоящий кросс-туловый редирект — Cursor-мост

`packages/coding-agent/src/cursor.ts`: фреймы провайдера Cursor транслируются в
ДРУГИЕ локальные тулы:

- `piFind` (поиск по именам файлов) → `glob` (`cursor.ts:822-828`);
- `piLs` → `read` (список директории, `cursor.ts:834-836`);
- `shell`/`piBash` → `bash` (`cursor.ts:592-600`, `747-752`);
- `piGrep` → `grep` с ремаппингом аргументов (`cursor.ts:791-808`).

Это честная замена «чужое имя вызова → наш тул», но она работает только на
wire-уровне Cursor-провайдера, а не для обычных tool-call'ов модели.

---

## 2. bash → dedicated tools: есть, но это БЛОК, а не редирект

`packages/coding-agent/src/tools/bash-interceptor.ts` (148 строк) —
`checkBashInterception()` (`:119-148`) возвращает
`{ block: true, message, suggestedTool }`. Вызывающий код
(`tools/bash.ts:1000-1010`) делает:

```ts
const interception = checkBashInterception(commandToCheck, ctx?.toolNames ?? [], rules, rawCommand);
if (interception.block) throw new ToolError(interception.message ?? "Command blocked");
```

То есть команда **не исполняется**, а модели возвращается ошибка с текстом
«Use the `grep` tool instead of grep/rg…». Никакого автовызова `grep` нет.
`docs/bash-tool-runtime.md:48` фиксирует это явно: «Interception and approval are
also separate mechanisms: interception routes misuse toward dedicated tools» — а
`:80` добавляет, что `suggestedTool` даже не доезжает до `details`, только текст.

Правила (`packages/coding-agent/src/exec/settings.ts:17-77`) — ровно то, о чём
спрашивали: `cat/head/tail` → `read` (`:21`), **`grep|rg|ripgrep|ag|ack` →
`grep`** (`:24-26`), `find|fd|locate` → `glob` (`:31`), `sed -i`/`perl -i`/
`awk -i inplace` → `edit` (`:36-46`), `echo … > file` → `write` (`:58`),
`nohup`/`&`/watch-режимы → `bash` с `name`.

Важные оговорки:
- **по умолчанию выключено**: `bashInterceptor.enabled` default `false`
  (`exec/settings.ts:147-157`);
- блок срабатывает только если suggested-тул есть в `ctx.toolNames`
  (`bash-interceptor.ts:130`);
- сегменты с piped stdin исключены (`bash-interceptor.ts:98-108`);
- правила целиком переопределяемы через `bashInterceptor.patterns`.

**Отдельно: bash → bash.** Есть один настоящий рерайт команды:
`rewriteGitWorktreeAdd` (`tools/bash-worktree-rewrite.ts:74-88`), который
подменяет `git worktree add …` на `omp worktree add …`
(`tools/bash.ts:1013-1015`, гейт `task.worktreeClone`). Это рерайт argv внутри
того же тула, не смена тула.

---

## 3. grep → find: стеринг только текстом

Перехвата нет. Что есть:

| Место | Текст |
|---|---|
| `prompts/system/system-prompt.md:114` | «Unknown behavior/location: descriptive `find` FIRST; NEVER guess `grep`/`glob` targets.» |
| `prompts/system/system-prompt.md:116` | «Regex/literal/known-symbol search: `grep`, NEVER shell `grep`/`rg`/`awk`.» |
| `prompts/tools/grep.md:4` | «`{{#if hasFind}}`Behavior/unknown symbol → `find`; literals/regex → `grep`.» |
| `prompts/tools/glob.md` | «Behavior search → `find`.» (условно, при `hasFind`) |
| `prompts/tools/find.md` | «MUST use first for unknown locations; known strings/regex/symbols → `grep`, names → `glob`.» |

Условия рендера считаются на лету в описании тула:
`tools/grep.ts:426-446` (`hasFind`, `eagerDelegation`, `scoutAvailable` →
битовая маска кэша → `prompt.render(grepDescription, …)`), аналогично
`tools/glob.ts:120-132`. `hasFind = session.isToolActive("find") ?? isFindEnabled(session)`,
а `isFindEnabled` (`tools/jfind/index.ts:46-50`) при `find.enabled: auto`
включает `find` только если judge-роль — нативная (`hasNativeJudge`).

Итог: omp **переписывает описания тулов** в зависимости от того, включён ли
`find`, но **не трогает сами вызовы**.

---

## 4. Промпт-стеринг в описаниях тулов

### 4.1. Negative-описания («don't use X for Y»)

`prompts/system/system-prompt.md:105-121` — блок `# Specialized Tools`,
целиком построен на «MUST use specialized tool over shell equivalent»:

```
- File/directory reads: `read`   - Surgical edits: `edit`
- Unknown behavior/location: descriptive `find` FIRST
- Regex/literal/known-symbol search: `grep`, NEVER shell `grep`/`rg`/`awk`
- File structure/names: `glob`, NEVER `ls **/*.ext`/`fd`
<critical>NEVER use `sed`|`perl`|`python` via `bash` to issue individual edits; MUST use `edit`.</critical>
```

Всё это обёрнуто в `{{#has tools "…"}}` — то есть вставки условные и исчезают,
если тула нет.

### 4.2. Порядок тулов и «инвентарь»

Порядок задаётся `BUILTIN_TOOLS` (`tools/index.ts:552-583`) и
`BUILTIN_TOOL_NAMES` (`tools/builtin-names.ts:1-31`) — `read` первым, `bash`,
`edit`, потом остальные. Специальной сортировки «по важности» нет; есть
order-preserving подпись `#computeAppliedToolSignature`
(`session/session-tools.ts:2127-2133`), чтобы любая перестановка вызывала
пересборку промпта.

Инвентарь каталога рендерится либо в системный промпт
(`renderToolInventory`, `packages/ai/src/dialect/inventory.ts:12-32` — формат
OpenAI-Harmony `namespace functions {…}`), либо остаётся в wire-схемах. Переключатель:
`config/inline-tool-descriptors-mode.ts:14-31` — `auto` = inline **только для
Gemini**, иначе off. Когда inline включён, `pruneToolDescriptions` вырезает
описания из wire-схем (`agent-loop.ts:1008-1042`), чтобы не дублировать.

### 4.3. Скрытие через описание (tool-level steering)

- `edit` — описание зависит от модели: `resolveEditToolDescription`
  (`edit/index.ts:158-170`) выбирает `edit-prompt-variant` из каталога
  (`compact` vs полный), а сам режим (`hashline`/`replace`) —
  `utils/edit-mode.ts:22-50` (kimi/mimo/minimax/deepseek/glm-flash → `replace`).
- `find`/`grep`/`glob`/`bash`/`eval` — условные ветки (`{{#if hasEval}}`,
  `{{#if hasFind}}`, `{{#if eagerDelegation}}`, `{{#if hasLaunch}}`).
- `task` — стеринг делегирования по модели: `resolveDelegationBias`
  (`catalog/src/compat/delegation.ts:44-86`, ось `delegation-bias` в
  `rules/classes/openai.kdl:44,49`) → `eager` / `restrained` / `gated`;
  используется в `task/prompt-policy.ts:10` и в описаниях grep/glob/ast_grep/eval.

### 4.4. Пост-фактум «редирект» через сообщения

- **TTSR** (`session/ttsr-coordinator.ts:185-197`): при совпадении ast-правила на
  аргументах вызова тула возвращает `{ block: true, reason }` и **прерывает
  генерацию**, инжектя правило. Это блок + инъекция, не редирект.
  Немигающий вариант — `additionalContext` с
  `prompts/system/ttsr-tool-reminder.md` (`ttsr-coordinator.ts:286`).
- **Loop guard** — `prompts/system/tool-call-loop-redirect.md:7`: «NEVER call
  `{{tool_name}}` with those arguments again this turn. Use different arguments,
  **choose another tool**…» (`session/stream-guards.ts:176-191`). Это ближайшее к
  «переключи тул», но опять текстом, без подмены.
- **Plan-mode reminder** (`prompts/system/plan-mode-tool-decision-reminder.md`) —
  «You MUST choose exactly one next action now: call `ask` OR write to
  `xd://propose`».

---

## 5. Динамическая видимость тулов

### 5.1. `loadMode` + `xd://` — главный механизм скрытия

`ToolLoadMode = "essential" | "discoverable"` (`agent/src/types.ts:1040-1050`).
`essential` — тул в top-level схеме; `discoverable` — вынимается из схемы и
монтируется как `xd://<name>` device, исполняемый через `read`/`write`
(`tools/xdev.ts:1-31`).

- Списки: `tools/essential-tools.ts:23-37` (read/write/bash/edit/glob/find/eval/
  task/wait/learn/manage_skill/context_notes/new_context).
- Классы: `grep` — `discoverable` (`tools/grep.ts:420`), `todo` — `discoverable`
  (`tools/todo.ts:725`), `glob`/`read`/`bash` — `essential`.
- Пиннинг: `XDEV_KEEP_TOP_LEVEL` (`tools/xdev.ts:58-64`) — `todo`, `yield`,
  `ask`, **`grep`**, `web_search` остаются top-level (комментарий `:50-57`
  объясняет: `grep` — цель редиректа bash-интерсептора). `read`/`write` —
  транспорт, никогда не монтируются (`:71-74`).
- Гейт: `tools.xdev` default `true` (`tools/settings.ts:912-924`).
- Монтирование: `session/session-tools.ts:1039-1046` (`mountCandidates`) →
  `isMountableUnderXdev` (`tools/xdev.ts:85-88`).
- Уведомления об изменении инвентаря — отдельным hidden-сообщением, чтобы не
  ломать prompt-cache: `prompts/system/xdev-mount-notice.md`,
  `prompts/system/tool-roster-notice.md` (`session-tools.ts:1497-1530`).

### 5.2. Скрытые и «по умолчанию неактивные» тулы

`HIDDEN_TOOLS = { think, yield, goal }` (`tools/index.ts:585-589`), плюс флаги
`hidden?: boolean` / `defaultInactive?: boolean` (`agent/src/types.ts:1136`).
`defaultInactive` не активируется, пока не запрошен явно
(`sdk.ts:4030-4061`, `:4822`).

### 5.3. Режимные профили (не «profiles» в смысле наборов тулов)

Отдельного понятия «tool profile» нет; `--profile` — это изоляция
конфига/сессий (`docs/settings.md:24-28`). Реальная динамика видимости:

| Механизм | Где | Эффект |
|---|---|---|
| Plan mode | `modes/interactive-mode.ts:4963-5010`; `task/structured-subagent.ts:215,250` | план-режим: `read, grep, glob, web_search` (+`ast_grep`), `write`/`edit` под `enforcePlanModeWrite` (`tools/plan-mode-guard.ts:129`) |
| Goal mode | `tools/index.ts:735-741` | `goal` только пока goal активен |
| `find.enabled` | `tools/settings.ts:521-531`, `jfind/index.ts:46` | `find` появляется только при нативном judge |
| Гейты per-tool | `tools/index.ts:730-795` | `bash.enabled`, `grep.enabled`, `glob.enabled`, `lsp.enabled`, `debug.enabled`, `ida` … |
| MCP | `session-tools.ts:2180+` | новые тулы приходят/уходят на `tools/list_changed` |
| Vibe mode | `session-tools.ts:773-796` | подменяет набор на «директорский» |
| Subagent `tools:` | `task/structured-subagent.ts:536` | `restrictToolNames` — жёсткий whitelist, auto-include выключен |

---

## 6. Что из этого переносимо (короткая мораль)

| Механика omp | Редирект вызова? | Стоимость воспроизведения |
|---|---|---|
| `beforeToolCall` / `tool_call` hook | **нет** — блок / args / context | низкая, но даёт только блок |
| bash-interceptor (grep→grep, cat→read…) | **нет** — `throw ToolError` с текстом | очень низкая: таблица regex→tool→message |
| `resolveToolForCall` + перепись `toolCall.name` | **да**, но только канонизация/алиас | средняя |
| Cursor exec-мост (`piFind`→`glob` и т.п.) | **да**, честный кросс-тул | средняя, привязана к wire-формату |
| `rewriteGitWorktreeAdd` | рерайт argv в том же туле | низкая |
| `loadMode`/`xd://` | нет, но скрывает схемы | высокая (транспорт read/write) |
| Условные вставки в описания | нет, промпт | очень низкая, высокая отдача |
| TTSR / loop-guard / plan-reminder | нет, блок + текст | низкая |

**Вывод:** если нужен именно редирект «модель позвала X → исполнили Y», в omp
готового примитива нет — придётся добавлять свой шаг между
`resolveToolForCall` и `prepareToolCallDispatch`, либо переиспользовать приём
Cursor-моста (таблица «wire-имя → локальный тул»). Всё остальное в omp —
это **блок с объяснением** плюс **текстовый стеринг**, и оба механизма уже
отполированы до состояния, которое дешевле скопировать, чем изобретать.
