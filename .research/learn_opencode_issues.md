# Уроки OpenCode (sst/opencode → anomalyco/opencode) для компрессии промпта allspark

Дата: 2026-10-19. Источники: свежий клон `/tmp/opencode-src` (dev, ba06ff9e, V2-архитектура),
GitHub issues/discussions `anomalyco/opencode` (репозиторий переехал с `sst/opencode`, ~57k issues).
Всё read-only, клон не менялся.

Ключевой вывод: OpenCode в V2 радикально сжал стационарную часть промпта (базовый system.txt —
756 байт, per-model добавки 300Б–8.7KB, итого ≤ ~9.5KB против наших ~30KB) и перенёс весь
динамический контент (AGENTS.md, скиллы, MCP, каталог тулов, дату, env) из системного промпта
в **аппенд-онли протокол дельт** поверх замороженного baseline — ради стабильности префикса кэша.
---

## Часть 1. ТОП-15 issues по влиянию

| # | Issue | State | Суть | Как починили/статус |
|---|---|---|---|---|
| 1 | #20647 Skills descriptions bloat system prompt (~600KB / ~150K tokens), breaking Bedrock | closed (dup #13188) | Все описания скиллов инжектились в system prompt каждый запрос: 1539 скиллов = 580KB ≈ 147k токенов, 56% окна контекста до первого сообщения; Bedrock (262k) отдавал BadRequest, fallback-цепочка исчерпывалась | V2: список скиллов вынесен из tool description в instructions-канал (дельты), полный текст грузится только через tool `skill` (один параметр `id`) |
| 2 | #13188 Lazy-load agent/skill lists instead of inlining into tool descriptions | closed | `task`/`skill` tool descriptions статически инлайнили полные списки агентов/скиллов каждый ход: 21 агент + 3 скилла ≈ 2000 токенов/ход; `init()` пересобирал описание каждый раз | V2: агенты — markdown-файлы через documents/instructions; скиллы — лёгкий индекс + lazy-загрузка инструментом |
| 3 | #14743 fix(cache): improve Anthropic prompt cache hit rate with system split and tool stability | closed (merged) | Anthropic хэширует tools→system→messages по порядку; нестабильный префикс = полный cache write на каждый первый промпт. Источники: единый system-блок с env/AGENTS.md; `Instance.directory` в схеме bash; несортированный порядок скилл-тулов | Split system на 2 блока (стабильный → динамический), cwd убран из схемы тулов, сортировка инвентарей; opt-in: заморозка даты, 1h TTL, cache audit |
| 4 | #51985 Image trimming breaks Anthropic prompt caching on every turn | **OPEN** | `boundImages` перекраивает историю каждый запрос (обрезка старых картинок 25MiB→15MiB): сдвиг cutoff меняет начало истории → полный cache write каждый ход. На реальном кейсе: $49 из $57 себестоимости — cache writes | Не починен; предлагается стабильный якорь обрезки (по номеру сообщения), а не пересчёт от начала |
| 5 | #55243 A fork of a fork misses the prompt cache: prompt_cache_key = intermediate fork's id | **OPEN** (TODO в коде `model-request.ts:294`) | `parentID ?? fork.sessionID ?? id` — внук шлёт ключ родителя, под которым никто не кэшировал → полный rewrite. Измерено: fork-of-fork = 0 cached tokens | Предложен walk до корня по parentID/fork-цепочке; ещё не влиты |
| 6 | #15457 System prompt mode switcher (Default/Light) | **OPEN** | Тяжёлый промпт (для frontier-моделей) съедает ~10% окна 32k локальной модели; избыточные правила деградируют рассуждение сильных моделей (#10927) | Предложен режим Light (минимальный промпт: тулы + среда + базовые правила); конфиг `prompt.mode`. Не сделан |
| 7 | #51098 Modernize and simplify the default system prompt | **OPEN** | `default.txt` в 2 раза больше `gpt-astra.txt`, накопил повторы: несколько правил «отвечай кратко», жёсткий лимит 4 строки, тотальный запрет комментариев, микро-правила батчинга | Предложено упростить до уровня astra-промпта; modern-моделям нужно меньше процедурного промптинга. Открыт |
| 8 | #10927 default system prompt materially degrades reasoning of high reasoning models | closed | CAPS-проклятие: «CRITICAL… STRICTLY FORBIDDEN… ZERO exceptions» каждый ход ломает CoT сильных моделей; напоминания-инъекции «это часть user msg» путают reasoning | Автор патчит сам; направление: убрать эмоционально-императивные блоки, короче и структурнее |
| 9 | #8277 / #12520 Lazy/dynamic loading for MCP tools; mcp-search tool | closed | Один MCP-сервер (GitHub) = 15–20k+ токенов одних схем; несколько серверов > 50k до первого сообщения | Итоговое решение V2 — **Code Mode**: один тул `execute`, MCP-тулы вызываются из кода, инлайн-каталог 2000 символов + `search()` внутри |
| 10 | #4773 Ability to hide subagents from primary agents system prompt | merged (63 коммента) | Список сабагентов разувал system prompt/тул-описание; нужны внутренние хелперы, невидимые юзеру | `hidden: true` + `permission.task` (allow/deny/glob); запрещённые сабагенты фильтруются из описания Task-тула, чтобы модель их не звала |
| 11 | #48844 "Session too large to compact" — the summarization request itself has no size budget | closed | Компакция не влезала в собственный запрос: (а) head без бюджета, (б) tail считался от окна компакционного двигателя, а не сессионной модели, (в) нет эскалации | Бюджет на весь summarization-запрос, tail от целевой модели, шаги уменьшения |
| 12 | #15533 Auto-compaction infinite loop when assistant ended its turn | closed | Синтетический «Continue...» инжектился безусловно при auto-compaction → цикл: ответ → overflow → компакция → Continue → … | «Continue» только при `finish === "tool-calls"`; при `stop` — выход цикла, ждать юзера |
| 13 | #54697 compaction request exposes session tools to the summarizer model | closed | Запрос компакции строился с тулами сессии: суммаризатор мог писать файлы и запускать команды вместо сводки | Запрос компакции собирается без тулов |
| 14 | #11313 Long-running bash commands: truncation → agent retry loops | closed | Обрезка вывода (2000 строк/50KB) без внятного указателя → агент перезапускал команды вместо чтения сохранённого вывода; side-effects, дубли работы | Полный вывод на диск + маркер с путём в выводе + обучение в description тула («полный результат сохранён в файл») |
| 15 | #18108 Truncated tool calls misclassified → repairToolCall doom loop | closed | JSON аргументов, обрезанный по maxOutputTokens, классифицировался как invalid tool call: без сигнала о трекации, цикл ремонта или тихий выход. Плюс OUTPUT_TOKEN_MAX=32k резал 128k-моделям, thinking-бюджет конкурировал с output | Определение `finish=length`, отдельная классификация, сигнал модели; пересмотр капов |

Также важны: #30649 (cache.read копится — счётчик сессии 56M токенов при 0.5MB данных → «unrecoverable session»), #18037 (AGENTS.md 331KB без лимита → компакция-луп на первом ходу), #4357 (конвертация схем теряла `.describe()` — модель слепа к параметрам), #22020 (глобальный AGENTS.md замолкает при наличии проектного), #8089 (auto-compaction всё равно ловит context_length_exceeded), #42935/#5422 (cache config провайдеров), #55041 (OpenAI explicit cache breakpoints, open), #20392 (закрытый ботом PR: guidance budget 32KB, mid-line ellipsis 4096B, CHARS_PER_TOKEN 3.5).

---

## Часть 2. ТЕХНИКИ OpenCode (файл:строка в /tmp/opencode-src)

### 2.1 Системный промпт: крошечный стабильный базис + per-model плагины
- Базовый промпт: `packages/core/src/session/runner/prompt/system.txt` (756 байт) — каркас, без env/даты/инвентарей. Подстановка `${OPENCODE_TOOL_GUIDANCE}` по включённым тулам: `packages/core/src/session/system-prompt.ts:5-38`.
- Per-model промпты-плагины: `packages/core/src/plugin/optimize.ts:15-52`. anthropic.txt — 300 байт, режим `"append"`; gpt.txt 2.6KB; gpt-astra 3.8KB; kimi 8.7KB. Тотальный размер ≤ ~9.5KB.
- Пер-модельная обрезка тулов: `optimize.ts:34-52` — `delete tools.grep; delete tools.glob` для GPT/Claude (пока не решён UX для тяжёлого grep/glob через shell).
- System в запросе = ровно 2 блока: `[agent.system | базовый промпт, initial]` — `packages/core/src/session/model-request.ts:110-118`.

### 2.2 Инструкции (AGENTS.md, скиллы, MCP, каталог, дата, env) — протокол «baseline + дельты»
- Источник-интерфейс: `packages/core/src/instructions/index.ts:26-45` — `key` + `read` (canonical JSON) + рендеры `initial/changed/removed`; хэш значений; `unavailable` (временный сбой чтения → стоит сохранённое значение) vs `removed` (наблюдаемое отсутствие).
- Все источники собираются в упорядоченный список **от самых общих к самым частным** — прямая оптимизация кэш-префикса, комментарий в коде: `packages/core/src/session/context.ts:155-161` (codemode-каталог → MCP → references → skills → AGENTS.md → date/env → session entries).
- Baseline эпохи рендерится один раз и **замораживается в durable event**: `packages/core/src/session/instruction-state.ts:45-47, 165-180`. Изменения после старта приходят в разговор отдельными delta-сообщениями, а не мутацией system.
- AGENTS.md грузится как synthetic-сообщение «Instructions from: <path>\n<content>» с dedup-леджером и повторной инжекцией, если компакция/revert выбросили его: `packages/core/src/session/instructions.ts:62-100`.
- Дата и env — тоже источники с дельтами («Today's date is now: …», `<env>` с cwd/root/git/platform/tmp): `packages/core/src/instructions/builtins.ts:22-50`.
- Лимит на значение инструкции: 256 KiB на запись — `packages/schema/src/instruction-entry.ts:30`.
- Discovery AGENTS.md: глобальный `~/.config/opencode/AGENTS.md` + проход вверх от cwd до корня + конфиг-паттерны (`globUp`): `packages/core/src/config/plugin/instruction.ts:38-56, 96-108`.

### 2.3 Скиллы: лёгкий индекс + ленивый тул + дельты + сортировка
- Тул `skill` с одним параметром `id`; полное содержание только по запросу: `packages/core/src/tool/plugin/skill.ts:12-27`.
- Индекс `<available_skills>` (id/name/description) в instructions-канале, отсортирован по id, исключает `autoinvoke: false`, фильтруется правами: `packages/core/src/skill/instructions.ts:17-30, 67-92`.
- Изменения списка рендерятся **дельтами** («New skills are available…», «…must not be used»), полная перепечатка только при структурных изменениях: `skill/instructions.ts:40-62`.
- Юзерский вызов скилла вставляет `<skill_content>` в разговор — модель не перечитывает: `skill/instructions.ts:22-24`.

### 2.4 Code Mode: сжатие схем тулов в код-подписи
- MCP-тулы по умолчанию (`codemode: true`) НЕ попадают в tool-list: один тул `execute`, вызов `tools.<server>.<tool>(...)` из кода: `packages/core/src/tool/mcp.ts:45`, `packages/core/src/codemode/instructions.ts:9-27`.
- Инлайн-каталог: бюджет 2000 символов (`CHARACTERS_PER_TOKEN=4`), первая строка описания обрезана до 120 символов, у каждого неймспейса счётчик «N tools, M shown» — `packages/core/src/codemode/catalog.ts:47-52, 160-172`.
- Полная видимость неймспейсов, полные листинги — «по одному неймспейсу за раунд, короткие сначала», остальное через `search(query, limit, offset)` с пагинацией (`remaining`, `next.offset`).
- Подписи в TypeScript-виде и лениво: `search` возвращает path+description+signature; рендер подписи только для совпадений поиска — `packages/codemode/src/tool-runtime.ts:108-112, 173-183, 282-292`.
- Туловые описания инструментов-«обёрток» учат модель правилам («Do not guess tool names», «search is synchronous»).

### 2.5 Кэш
- `promptCacheKey` = session affinity (id/родитель), с TODO о lineage для внуков: `packages/core/src/session/model-request.ts:293-294`.
- Провайдерский плагин включает `supportsPromptCacheKey` для chat-completions провайдеров с аудитой по каждому: `packages/core/src/plugin/provider/prompt-cache-key.ts:7-40`.
- Уроки #14743: стабильный порядок инвентарей, никакой динамики в первом system-блоке, cwd не в схемах тулов; opt-in флаги `OPENCODE_EXPERIMENTAL_CACHE_STABILIZATION` (заморозка даты), `OPENCODE_EXPERIMENTAL_CACHE_1H_TTL`, `OPENCODE_CACHE_AUDIT`.

### 2.6 Обрезка выводов тулов
- `MAX_LINES=2000`, `MAX_BYTES=50KiB`; полный вывод на диск (`~/.opencode/data/tool-output/tool_*`, retention 7 дней); маркер в выводе: `[showing lines 1-N of M; full output saved to <path>]` — `packages/core/src/tool-output.ts:15-17, 76-110`.
- Description тула shell явно учит: «When output is large, the full result is saved to a file and a truncated preview is returned. Rely on automatic truncation unless filtering the output is more useful» — `packages/core/src/tool/plugin/shell.ts:42-43`.
- Картинки: бюджет 25MiB trigger → 15MiB target, старые заменяются текстовой заметкой: `packages/core/src/session/model-request.ts:43-45, 189-214` (но см. граблю №4).

### 2.7 Компакция
- Ceiling = окно − max(10% окна, RESERVE_MIN); unknown window = 200k; auto при `estimate >= ceiling`: `packages/core/src/session/compaction.ts:911-922, 242-252`.
- Эскалация при отказах провайдера: `SHRINK_STEPS=[0.7,0.5,0.35]`, при overflow первый ретрай сразу aims ниже: `compaction.ts:95, 227-233, 437-444`.
- Fallback `flattenAndDropOldest`: прошлый summary остаётся, обмены (user→до следующего user) выбрасываются целиком старые-первый, транскрипт без reasoning/media/instruction-updates, длинные tool outputs режутся — `compaction.ts:466-519, 791-825`.
- Промпт суммаризатора прямо запрещает пересказывать статический контент («Leave out … instruction files such as AGENTS.md, and environment details … The next agent receives current versions of all of these separately») и требует шаблон; NUDGE при несоблюдении; миграция legacy-чекпоинтов, которые разрастались: `compaction.ts:127-190`.
- Запрос компакции без тулов (#54697), без system-промпта сессии (#43247 — отдельный баг: PROMPT_COMPACTION не доставлялся), доверие только счётчикам текущего провайдера (`hasMeasuredPrompt`, `compaction.ts:930-940`).

---

## Часть 3. ГРАБЛИ: топ-10, которые нам не повторить

1. **Инвентари в промпте без бюджета** (#20647, #13188, #18037). 580KB скиллов / 331KB AGENTS.md → 400 от Bedrock, компакция-луп на первом ходу. → Наш cap скиллов 30 держать, но добавить **байтовый бюджет** на каждый инвентарь и **обязательный репорт обрезки** (наше же правило prompt-input caps). Ничего не резать молча.
2. **Недетерминированный порядок инвентарей** (#14743 п.3). `Object.values()` на glob → другой порядок → cache miss после рестарта. → Сортировать всё: тулы, скиллы, MCP, AGENTS.md-цепочку. У нас ростер сортирован — расширить на skills/instructions.
3. **Динамика в стабильном префиксе**: cwd в схеме bash (#14743 п.2), дата, env. → Аудит префикса: одна строка с путём/датой = инвалидация всей сессии. Env — во второй блок или дельтой.
4. **Пересчёт переписываний истории** (#51985): обрезка картинок от начала каждый запрос → полный cache write каждый ход ($49/$57). → Любая выгрузка/обрезка — по стабильному якорю (номеру сообщения), решение фиксируется, не пересчитывается.
5. **Ключ кэша не от корня** (#55243): fork-of-fork и вложенные сабагенты шлют ключ, под которым никто не писал → полные rewrite. → `prompt_cache_key`/affinity наследовать от корневой сессии; TODO OpenCode confirmation, что это реальная боль.
6. **Компакция как self-DoS** (#48844, #15533, #30680, #54697, #43247): суммаризатор без бюджета; tail от окна не той модели; синтетический «Continue» → вечный луп; тулы в суммаризаторе; system не доставлен. → Каждую guards-ветку покрыть тестом: бюджет суммаризации от сессионной модели, эскалация 0.7/0.5/0.35, «Continue» только при tool-calls, запрос компакции без тулов, доставленный компакционный промпт.
7. **Обрезка без действенного указателя** (#11313): агент перезапускал команды. → Маркер обязан содержать путь к полному выводу; туловые descriptions обучают («полный вывод сохранён в файл, читай, не перезапускай»); полный вывод на диске с retention.
8. **Обрезанный tool-call JSON → doom loop** (#18108): finish=length классифицировался как invalid call; низкий cap выхода усугублял. → Детектировать length на tool-call, подавать модели сигнал, не резать max output до 32k «на всякий случай».
9. **Потери при конвертации схем** (#4357, #11357, #31996): `.describe()` терялся в zod→JSON; пустая схема `{}` невалидна в strict-mode; regex lookaround не поддержан у части провайдеров. → Один конвертер схем, wire-тесты на каждого нашего провайдера (у нас уже есть provider-wire tier — использовать).
10. **Ненасытные счётчики и ретеншн** (#30649, #33356): cache.read копится (56M токенов «в сессии»), event-таблица 13GB от повторных снапшотов → OOM на старых сессиях. → Кумулятивные метрики ограничивать/разделять (per-turn vs total), retention на артефакты (у нас session-artifacts — заложить политику сразу).

---

## Часть 4. ЧТО СКОПИРОВАТЬ в allspark (Rust)

| Механизм | Референс | Зачем | Оценка |
|---|---|---|---|
| Протокол инструкций «baseline + дельты» (key+hash dedup, initial/changed/removed, re-inject после компакции) | `instructions/index.ts:26-45`, `instruction-state.ts:45-47`, `session/instructions.ts:62-100` | Стабильный префикс + неизменность AGENTS.md/скиллов не ломают кэш; у нас это главный отсутствующий примитив | 16–24 ч |
| Перенос AGENTS.md/env/даты/скилл-листа из system-блока в этот протокол; ordering most→least shared | `session/context.ts:155-161`, `builtins.ts:22-50` | Наши динамические вставки перестанут инвалидировать префикс | 8–12 ч (после п.1) |
| Маркер обрезки вывода с путём к файлу + обучение в description тула | `tool-output.ts:15-17,76-110`, `shell.ts:42-43` | Дешёво, убирает re-run-лупы | 4–6 ч |
| Guardrails компакции: суммаризатор без тулов, бюджет от сессионной модели, SHRINK_STEPS, flatten-drop-oldest, «Continue» только при tool-calls, запрет пересказа статического контента в промпте суммаризатора | `compaction.ts:95,127-190,227-233,466-519` | Наш план компрессии не спасает длинные сессии без этого | 10–14 ч |
| Root-derived cache key для форков/сабагентов | `model-request.ts:293-294`, issue #55243 | Прямая экономия на каждом fork/subagent | 2–4 ч |
| Стабильный якорь для image/media-бюджета | `model-request.ts:43-45` | Не повторять $49-кейс | 4 ч |
| Code Mode-каталог для MCP (2k инлайн + `search` + `execute`) — фаза 2; дешёвая версия: ленивый `mcp_search`-тул из #12520 | `codemode/catalog.ts:47-52`, `codemode/instructions.ts:9-27`, `tool-runtime.ts:108-112` | MCP-серверы у нас съедают десятки k токенов схемами | 24–40 ч (полный) / 8–12 ч (ленивый тул) |
| Per-model «лёгкий» промпт-режим (опция, как #15457) + пер-модельная обрезка тулов как в `optimize.ts` | `optimize.ts:15-52` | Локальные/малые модели и абляция | 4 ч |

Рекомендованный первый срез: п.1 + п.2 + маркер обрезки + cache-key lineage ≈ 30–46 ч,
даёт наибольшую экономию на кэше при минимальном риске. Компакция — вторым срезом, Code Mode — третьим.

---

## Часть 5. Сравнение с нашим планом

Наш план: абляция; cap скиллов 30; перенос сигнатур в схемы; кумулятивный бюджет AGENTS.md; LLMLingua на tool outputs.

**Совпадает с опытом OpenCode:**
- **Абляция** — подтверждена дважды: их стационарный промпт ужался до ≤9.5KB, а #51098/#10927 просят ещё короче: повторы и CAPS-императивы деградируют сильные модели. Наш 30KB — очевидный запас.
- **Перенос сигнатур в схемы** — OpenCode пошёл дальше (Code Mode: сигнатуры одной строкой, 2000-символьный инлайн-бюджет, `search` для остального). Направление верное, добавить бюджет и пагинацию.
- **Кумулятивный бюджет AGENTS.md** — подтверждён (#18037: 331KB без лимита = компакция-луп; у них 256KiB/запись в V2). Совпадает с нашим лимитом 256KiB на промпт-бомбы.

**Противоречит / требует поправки:**
- **Cap скиллов 30 (по количеству)**: OpenCode не режет по количеству — они делают запись дешёвой (1 строка, отсортировано, дельты при изменениях) и тяжёлое прячут за тул. Жёсткий cap молча прячет функциональность; лучше **байтовый бюджет + репорт обрезки** (наше правило prompt-input caps) + ленивая загрузка. Cap оставить как аварийный предохранитель.
- **LLMLingua на tool outputs**: OpenCode нигде не «сжимает» выводы семантически — только структурная обрезка head-keep + полный вывод в файл. Компакционный промпт требует сохранять точные строки (пути, команды, ошибки) — LLMLingua портит именно это. Риск: модель получает изменённые идентификаторы кода. Рекомендация: LLMLingua не для туловых выводов вообще; если очень нужно — только для прозы (логи обсуждений), никогда для кода/диффов. Базовый механизм — их: лимиты + файл + маркер.

**Чего в нашем плане не хватает (по важности):**
1. Протокол «baseline + дельты» для всех источников инструкций — у нас перенос сигнатур и бюджеты, но нет механизма, сохраняющего префикс при изменениях скиллов/AGENTS.md/MCP (у OpenCode это ядро V2).
2. Ordering источников most→least shared для кэш-префикса.
3. Cache-key lineage для форков/сабагентов (у нас ростер отсортирован, но ключ производный от текущей сессии — та же ошибка #55243 вероятна).
4. Стабильные якоря для любых переписываний истории (медиа-бюджет, выгрузка старых сообщений).
5. Guardrails компакции из Части 4 п.4 — у нас в плане компакция не упомянута, а именно она чаще всего «ест» токены впустую (лупы, непопадание в окно).
6. Действенный маркер обрезки вывода (путь к файлу) + обучение в туловых descriptions.
7. Схемные wire-тесты на потери описаний/strict-mode/#31996-регексы — у нас есть provider-wire tier, покрыть конвертер схем.
8. Ретеншн-политика на session-artifacts и ограничение кумулятивных счётчиков (#30649, #33356).
