# Описания тулов-графов: как gitnexus и codebase-memory заставляют модель ВЫБИРАТЬ граф

**Дата:** 2026-10-19 · **Режим:** read-only разбор + реврайт наших текстов
**Вопрос:** почему агент в реальной сессии вызвал `python_repl` 132×, `lsp` 15×, `graph` 1× — и что должно быть написано в описаниях, чтобы граф выбирали.
**Объекты:** MCP-серверы `gitnexus` (17 тулов) и `codebase-memory-mcp` (17 тулов), подключённые в этой сессии; наши `crates/pa-core/src/tools/graph.rs`, `crates/pa-lsp/src/tool.rs`, `crates/pa-core/src/prompts/system_prompt.rs::native_tools_section`.
**Связные отчёты (не дублируются):** `learn_blackbox_qa.md` (ST-задачи, оракулы), `learn_prompts_caching.md` (oh-my-pi держит tool-промпты в `description` схем), `learn_caching_2026.md` (прогрессивное раскрытие, дедупликация system↔tool).
**Сырые данные:** `/tmp/roadmap_audit/raw_tool_descs/{gitnexus,codebase_memory}.json`; полные тексты — в приложениях 1–2 этого файла.

---

## 0. TL;DR

1. **Триггер важнее перечня действий.** gitnexus в 16/17 описаний начинает с `WHEN TO USE` — фразы в голосе пользователя («Debugging "how does A reach B?"», «Renaming a function…», «Before committing»). Модель маршрутизирует по совпадению «текущая задача ↔ триггер-фраза». Наше описание graph — это меню (`Actions: index, reindex, search…`): совпадать не с чем.
2. **Якорь стоимости.** gitnexus почти всегда quant-оценивает наивный путь: «answers in one call what would take **3-8 manual context/impact hops**», «**Safer than** find-and-replace», «not just file matches». Это единственное предложение, которое убеждает модель, что граф ДЕШЕВЛЕ её дефолта (grep+read).
3. **Позиция.** В «маршрутообразующих» тулах триггер стоит в первых ~100–200 символах (trace — 87-й, api_impact — 52-й, cypher — 56-й). Триггер в конце описания — как наш единственный «MUST use it for architectural questions» в конце 835-символьного блока — не работает.
4. **Два полюса объёма.** gitnexus: медиана **1 622 симв. (~405 tok)** на тул; codebase-memory: медиана **140 симв. (~35 tok)**, но смысл унесён в описания параметров. Наш graph: **453 симв.** в схеме (≈113 tok) — в 3.5 раза ниже медианы gitnexus, и всё это меню действий.
5. **Ноль сценариев.** В наших schema-описаниях graph/lsp нет ни одной вопросной фразы («who calls», «what breaks», «where is defined»). gitnexus цитирует вопросы пользователя дословно в 5 тулах.
6. **Связки.** 8/17 тулов gitnexus имеют `AFTER THIS:` — явный workflow-DAG (query→context→impact; rename→detect_changes). Наш `graph` не участвует ни в одной цепочке: после «нашёл символ» и «перед коммитом» никаких указаний нет.
7. **Анти-паттерны.** gitnexus: 0 заглавных NEVER, вместо этого позитивный роутинг «prefer api_impact for pre-change» (дважды — осознанное deprecating слабых тулов) и семантика ловушек («changed_count:0 with partial:true is **NOT** a clean pre-commit check»). Мы: 7 заглавных MUST/NEVER — но для graph единственный анти-паттерн («reading file after file») спрятан в passive-придаточное в конце блока, а реальный наблюдаемый провал — 132 вызова python_repl — не покрыт вообще.
8. **Размазывание по слоям.** Мы дублируем список действий в schema И в system prompt. learn_caching_2026 (антропиковский принцип дедупликации): инструкция живёт в tool description. У нас сильный текст — в system prompt, слабый — в схеме, а роутинг происходит на уровне списка тулов, рядом с `python_repl` («Execute Python code…» — универсальный триггер без границ, выигрывает всё).

---

## Часть 3. РАЗБОР ПРИНЦИПОВ

### A. Прогрессивное раскрытие: от вопроса к тулу

Описание gitnexus — шестиуровневая структура, где уровни 1–3 (маршрутизация) стоят в первых ~20% текста, а механика — после:

| # | Уровень | Маркер | Пример (trace) |
|---|---|---|---|
| 0 | идентификация | первое предложение | «Find the shortest directed path between two symbols over call and class-member edges.» |
| 1 | триггер задачи | `WHEN TO USE:` | «Debugging "how does A reach B?"» |
| 2 | якорь стоимости | тире-придаточное | «— answers in one call what would take 3-8 manual context/impact hops» |
| 3 | следующий шаг | `AFTER THIS:` / `READ gitnexus://…` | (у trace нет; у query: «Use context() on a specific symbol for 360-degree view») |
| 4 | механика вывода | `Returns:` | «ordered hops with file:line, and an aligned edges[] of edge type + confidence» |
| 5 | семантика ловушек | `Handles disambiguation` / `partial:` / `epistemic` | «totalCandidates — the TRUE match count, not candidates[].length» |
| 6 | советы/примеры | `TIP:` / `EXAMPLES:` | (cypher: 10 готовых Cypher-запросов) |

Формулировки-маршрутизаторы, встреченные в корпусе:

- **Вопросные цитаты** (голос пользователя, дословно): `trace` — «Debugging "how does A reach B?"»; `pdg_query` — «"under what condition does X run?"», «"where does variable Y flow?"», «"what guards this statement?"»; `explain` — «"what taint findings exist in this repo / file / function?"». Модель сопоставляет вопрос юзера с цитатой почти дотошно.
- **Временные триггеры** (когда в жизненном цикле работы): «Before committing — to understand what your changes affect. Pre-commit review, PR preparation» (detect_changes); «Before making code changes — especially refactoring, renaming, or modifying shared code» (impact); «BEFORE modifying any API route handler» (api_impact, капс = приоритет); «After changing group.yaml or re-indexing member repos» (group_sync).
- **Якорь стоимости / сравнение с альтернативой:** «answers in one call what would take 3-8 manual context/impact hops» (trace); «Safer than find-and-replace» (rename); «Use this when you need execution flows and relationships, not just file matches. **Complements** grep/IDE search» (query — очерчивает НЕэксклюзивную территорию, снимает страх «а вдруг grep был правильнее»).
- **Позиция:** trigger в первых 200 символах у trace (87), api_impact (52), cypher (56), shape_check (82), detect_changes (150), query (169). Исключения — служебные тулы (list_repos 62%, group_list 50%), которым маршрутизация не критична.
- **Дешёвость попытки:** `explain`/`pdg_query` — «without that layer the tool returns a clear "no taint layer" note, **not an error**». Сообщение «попробовать — безопасно» систематически повышает выбор тула в серых зонах.
- **Первый шаг:** list_repos — «First step when multiple repos are indexed». Явное «с чего начать».

Сколько уровней: 6–7, но маршрутная нагрузка на уровнях 1–3; уровни 4–6 можно сворачивать (что и делает codebase-memory, см. ниже).

### B. Сценарии, а не список действий

gitnexus описывает **задачи пользователя**, а не кнопки. Даже самый «кнопочный» tool_map получает сценарий: «Understanding tool APIs, finding tool implementations, impact analysis for tool changes». Триггерная фраза всегда — про ЦЕЛЬ, а не про механику: не «runs BFS over CALLS edges», а «Debugging "how does A reach B?"».

Наше описание graph — сплошная механика: «Actions: index/reindex (build or incrementally repair the symbol/call/import graph), search (hybrid symbol search…)…». Пользовательских вопросов в нём ноль — проверка по словарю сценариев (who calls, what breaks, reach, blast radius, before committing) даёт **0 попаданий** в schema-описаниях graph и lsp. Единственное сценарное «architectural questions and call-chain questions» сидит в system-prompt-блоке, в последнем предложении, в придаточном.

Codebase-memory — контрольная группа: 140-символьные описания-контракты («Trace callers/callees, data flow, or cross-service paths. Defaults exclude tests and resolver evidence.») — маршрутизации нет вовсе; тул выбирается по имени и по описаниям параметров. Работает хуже как.router — именно поэтому его и стоит копировать только в части «контракт в параметрах», не в части «описание-телеграмма».

### C. Анти-паттерны в описании

Три механизма, от сильного к слабому:

1. **Позитивный пере-роутинг** (`prefer <лучшая> альтернатива`): route_map и shape_check оба пишут «For pre-change analysis, **prefer `api_impact`** which combines this data with mismatch detection and risk assessment». Слабые тулы сами отправляют на композитный. У нас аналогичная пара есть (graph vs lsp для навигации) — но она не сформулирована.
2. **Запрет в триггере**, а не отдельной секцией: у query запрет вшит в WHEN TO USE — «when you need execution flows and relationships, **not just file matches**». Запрет стоит рядом с триггером, а не paragraphs ниже.
3. **Заглавные MUST/NEVER** (стиль Claude Code): в gitnexus **0** вхождений — весь запретительный аппарат сделан через (1) и (2). У нас в native_tools_section 7 MUST/NEVER, и они работают для lsp/edit (ST-001, ST-003 проходят), но для graph анти-паттерн один и спрятан: «MUST use it for architectural questions and call-chain questions **instead of reading file after file**» — passive, в хвосте, без якоря стоимости рядом.

Критический пробел: наблюдаемый анти-паттерн сессии — **132 вызова python_repl** (ручные grep/read-петли в REPL) — не назван нигде. Единственная фраза про python_repl в секции: «`python_repl` **remains the right tool** for orchestration…» — начинается с подтверждения дефолта, т.е. якорит в обратную сторону.

### D. Связки между тулами

gitnexus строит явный DAG из 8 рёбер `AFTER THIS`:

```
list_repos → context(repo)          query → context(symbol)
cypher  → context(symbol)           context → impact (если планируем правки)
rename  → detect_changes (проверка) impact → context(high-risk) + review d=1
detect_changes → context + process  route_map → impact
```

плюс 6 ресурсных указателей `READ gitnexus://repo/{name}/…`, плюс **повторяемый дословно** контракт неоднозначности в 6 тулах («totalCandidates — the TRUE match count, not candidates[].length… Re-call with uid») — одно и то же предложение формирует одну ментальную модель на весь набор.

У нас в «Working with these tools» есть связки для lsp→edit («run `lsp` `references` first…»), но **graph не входит ни в одну цепочку**: нет «trace→читай тело lsp», нет «`changes` перед коммитом», нет «перед rename — graph trace/references». Тул изолирован, и workflow-памяти о нём не создаётся.

### E. Числа

| Метрика | gitnexus | codebase-memory | наше graph | наше lsp |
|---|---|---|---|---|
| Описаний | 17 | 17 | 1 (schema) + 1 (sys) | 1 (schema) + 1 (sys) |
| Медиана описания | **1 622 симв. (~405 tok)** | 140 (~35 tok) | 453 (~113 tok) | 437 (~109 tok) |
| Максимум | impact 15 445 (выброс: контракты PDG) | query_graph 336 | — | — |
| WHEN TO USE | 16/17 | 0/17 | 0 | 0 |
| Триггер в первых 200 симв. | 12/16 | — | 0 | 0 |
| AFTER THIS / цепочки | 8/17 (+6 ресурсных) | 0 | 0 | 0 |
| Примеры вызовов | cypher: 10 Cypher-примеров | 0 | 0 | 0 |
| Вопросные цитаты | 5 тулов | 0 | 0 | 0 |
| Отношение «когда»/«как» | ≈15–25% / 75–85% | ≈0/100 | 0/100 | 0/100 |
| Заглавные MUST/NEVER | 0 | 0 | 0 (в sys-блоке: 2) | 0 (в sys-блоке: 2) |
| Описания параметров | полные, с уроками («task_context: Helps ranking») | несут основной контракт | «The graph operation to run.» (14 симв. на 8 действий) | — |

Замечание про цену: реврайт до gitnexus-медианы добавит ~1.2k симв. на тул в каждый запрос → в static-префикс (кэшируемый). При 200-ходовой сессии это ~0.6k tok × 200 × $0.1/1e6 (cache read) ≈ копейки; измерить дельту `prime-agent prompt --json` до/после.

### F. Сравнение с нами: почему модель игнорирует наш graph

1. **Голод триггера.** 453 символа — и ни одной фразы, к которой цепляется вопрос пользователя. ST-002 спросил «Trace the full call chain from main…» — слово «trace» совпало с именем действия, но вопросная форма («call chain») встречается в наших текстах один раз, в хвосте system-блока.
2. **Нет якоря стоимости.** Нигде не сказано «один вызов вместо N открытий файлов» рядом с триггером. gitnexus делает это в каждом маршрутном описании.
3. **Не покрыт реальный провал.** 132 вызова python_repl = ручные цепочки чтения. Ни одна наша строка не говорит «не собирай call chain вручную через python_repl/grep — это работа graph.trace».
4. **Слои перепутаны.** По принципу дедупликации (learn_caching_2026 §2.6) инструкция должна жить в tool description — она едет с каждым запросом и стоит в списке тулов, где и происходит выбор. У нас сильный текст — в system prompt (835 симв., которые читаются как общая политика), слабый — в схеме (меню). Роутинг проигрывается на уровне списка тулов против `python_repl` («Execute Python code…» — триггер без границ).
5. **Ноль связок.** graph не в одном AFTER THIS; нет «перед коммитом — changes», нет «trace → lsp читает тело».
6. **`action`-параметр не маршрутизирует.** «The graph operation to run.» — 14 символов на 8 действий. gitnexus кладёт уроки в параметры («What you are working on… Helps ranking»); у нас параметр мог бы нести мини-триггеры на каждое действие.

### Что переписать (сводка)

- graph.rs `fn description`: меню → триггеры + якорь стоимости + цепочка (см. Часть 4).
- graph.rs `fn params_schema().action`: 14 символов → мини-триггер на каждое действие.
- system_prompt.rs `native_tools_section()`: блок graph — сценарии вперёд, действия после, анти-паттерн python_repl назван явно; поправить python_repl-предложение, которое якорит дефолт; добавить graph в workflow-цепочки.
- lsp: schema-описание получает триггеры (сейчас их 0); system-блок restructuring: триггер → действия → контракт доверия.

---

## Часть 4. ПЕРЕПИСАННЫЕ ОПИСАНИЯ ДЛЯ НАС (готовые тексты, английский)

Замена по образцу gitnexus: (1) триггер-вопросы в первых 200 символах, (2) якорь стоимости рядом, (3) цепочка к следующему тулу, (4) механика после маршрутизации, (5) ловушки переименованы в семантику результата. Дублирование schema↔system-prompt устраняем: схема несёт триггеры+механику, system-блок — политику и анти-паттерны.

### 4.1 `graph` — schema description (`crates/pa-core/src/tools/graph.rs`, `fn description`)

```rust
fn description(&self) -> &'static str {
    "Query the code graph of the working directory: symbols, caller/callee and \
     import edges, architecture views, git-diff impact. WHEN TO USE: any question \
     about how code is connected — 'who calls X?', 'trace the path from A to B', \
     'what breaks if I change X?', 'which modules does this file reach?', 'where \
     are the cycles/hotspots/entry points?', 'what did my diff touch?' — and \
     before refactoring or deleting shared code, to size the blast radius. One \
     query replaces grepping a name and reading every hit: text search cannot see \
     call edges. AFTER THIS: read just the bodies you found with `lsp`, and run \
     `changes` before committing. Actions: search (hybrid symbol search — describe \
     behavior in words, not a regex), trace (callers, callees, shortest path from \
     a symbol; direction, depth, edge_types), architecture (view: \
     overview/structure/layers/clusters/cycles/hotspots/entry_points/file_tree), \
     changes (git diff → affected symbols), coverage (index honesty report), \
     index/reindex (search, trace, and architecture index on first use — no \
     cold-start step), status."
}
```

(~1 240 симв. ≈ 310 tok — в пределах медианы gitnexus 1 622; триггер на 171-м символе.)

### 4.2 `graph` — параметр `action` (`fn params_schema`)

```rust
"action": {
    "type": "string",
    "enum": [ "index", "reindex", "search", "trace",
              "architecture", "coverage", "changes", "status" ],
    "description": "Graph operation. search: find symbols by describing behavior \
                    in words. trace: 'who calls X', 'how does A reach B' — \
                    callers/callees/paths from a symbol. architecture: layers, \
                    cycles, hotspots, entry points. changes: which symbols a git \
                    diff touches — run before committing. coverage: index honesty \
                    report. index/reindex: build or repair the graph (automatic \
                    on first search/trace/architecture). status: store counts."
}
```

### 4.3 `graph` — блок в `native_tools_section()` (system prompt)

```rust
"- `graph`: relationship queries over the working directory — the only tool that \
 already holds the caller/callee, import, and containment edges. Use it whenever \
 the question is about how code is CONNECTED, not what one file contains: 'who \
 calls this?', 'how does A reach B?', 'what breaks if I change X?', 'what does \
 my diff affect?', 'where are the cycles, hotspots, entry points?'. MUST use it \
 for these questions instead of reading file after file, grepping a name and \
 opening every hit, or walking a call chain by hand in `python_repl` — the graph \
 answers in one call what takes 3-8 manual reads, and text search misses \
 callsites hidden behind re-exports and dynamic dispatch. Actions: `search` \
 (hybrid symbol search — describe behavior in words, not a regex), `trace` \
 (callers/callees/shortest path from a symbol; `direction`, `depth`, \
 `edge_types`), `architecture` (`view`: \
 overview/structure/layers/clusters/cycles/hotspots/entry_points/file_tree), \
 `changes` (git diff → affected symbols — run it before committing), `coverage` \
 (index honesty report), `index`/`reindex`, `status`. `search`, `trace`, and \
 `architecture` index the project on first use, so there is no cold-start step. \
 Workflow: `graph` `search`/`trace` to find the symbols → `lsp` to read or edit \
 one body → `graph` `changes` before commit."
```

### 4.4 `lsp` — schema description (`crates/pa-lsp/src/tool.rs`, `fn description`)

```rust
fn description(&self) -> &'static str {
    "Symbol-level code intelligence from real language servers plus a tree-sitter \
     layer: definitions, references, implementations, hover, diagnostics, \
     document/workspace symbols, renames, and symbol-addressed edits. WHEN TO \
     USE: 'where is X defined?', 'who references X?', 'what does this override \
     or implement?' — a text search misses shadowing, re-exports, and cross-file \
     callsites; before renaming or deleting an exported symbol (one call updates \
     every callsite); to read or replace a single method body without opening \
     the whole file. Address a symbol by a `Class/method` name path or a 1-based \
     line."
}
```

(~690 симв.; триггер на 260-м символе — сразу после перечисления возможностей, которое нужно для дизамбигуации с graph.)

### 4.5 `lsp` — блок в `native_tools_section()` (system prompt)

```rust
"- `lsp`: symbol-level code intelligence from real language servers plus a \
 tree-sitter layer. Use it for every 'where is it defined / who references it / \
 what does it override' question, and to read one symbol instead of a whole \
 file: a text search misses shadowing, re-exports, and cross-file callsites, \
 and a `sed` rename silently drops them. MUST use it for definitions, \
 references, implementations, hover, and renames; NEVER text-search for code \
 intelligence. Actions: `definition`/`declaration`/`type_definition`/\
 `implementation`, `references`, `hover`, `diagnostics` (file, glob, or \
 workspace), `symbols` (file overview or workspace search), `find_symbol`, \
 `find_referencing_symbols`, `find_implementations`, `rename`/`rename_file`, \
 `code_actions` (imports, quick fixes, server-known refactors — reach for it \
 before editing by hand), `reload`, and symbol-aware edits \
 (`replace_symbol_body`, `insert_before_symbol`/`insert_after_symbol`, \
 `safe_delete_symbol`). Address a symbol by a `Class/method` name path \
 (`symbol`) or a 1-based `line`. A successful `rename`/`rename_file`/\
 `safe_delete_symbol` is already complete — trust it, do not re-read changed \
 files to confirm. Chain: `graph` `trace` finds the callsites, `lsp` reads and \
 edits the bodies."
```

### 4.6 Попутные правки в `native_tools_section()` (не про graph напрямую, но тот же баг-класс)

- Существующее предложение якорит дефолт: «`python_repl` remains the right tool for orchestration…» — модель читает первое полупредложение как разрешение. Заменить на:

```
"- Route code questions to the specialized tool first; `python_repl` is for \
 orchestration, data transformation, and computation — call these tools from it \
 when helpful, but do not rebuild their answers by hand with grep/read loops."
```

- В раздел «Working with these tools» добавить две связки (сейчас graph вне цепочек):

```
"- Before committing, run `graph` `changes` to see which symbols the diff \
 touches beyond the files you edited.",
"- Division of labor: `graph` answers relationship questions (calls, paths, \
 architecture); `lsp` answers symbol questions (definition, references, body). \
 `grep`/`bash` text search is the fallback for string literals and comments."
```

---

## Часть 5. ЧТО ПРОВЕРИТЬ ТЕСТОМ

Слои — по матрице из AGENTS.md (дешевейший тир, который может упасть):

1. **Unit (ms):** контракт prompt-слоя. Отрендерить description graph/lsp и ассертить структуру, не содержимое: содержит `WHEN TO USE`, длина в коридоре (например 800–2 000 симв. для graph), параметр `action` содержит `before committing`. Это не «тест на магические строки» — строка и есть продукт (промпт-контракт), ассертим её форму, чтобы никто не откатил к меню действий.
2. **QA faux (~30 s):** ST-002 уже оракулит `must: graph, args_regex "trace|search|architecture"`. Оставить как есть; добавить симметричную задачу ST-0XX «blast radius»: промпт «You renamed `validate_order` to `check_order` by hand in one file. Which other callers still use the old name? Report them.» — ловушка: правка уже внесена вручную, grep по имени даёт хвост, а честный ответ требует `graph changes`/`trace`. `must: graph, args_regex "changes|trace|search"`.
3. **QA real-model (free-tier, минуты):** главный верификатор реврайта — дифференциальный A/B на ST-002: старое описание vs новое, один и тот же промпт, N≥5 прогонов на ветку, метрика — доля прогонов с ≥1 вызовом graph + гистограмма tool-вызовов. Красный контроль: на старом описании доля < 1.0 (задокументированный живой случай — graph 1× за сессию). Без этого A/B реврайт — гипотеза, а не фикс.
4. **Анти-паттерн python_repl:** если lane поддерживает счётчики — кап `python_repl` ≤ 10 на ST-002; если нет — `must_not` на python_repl `args_regex` вида `for .* in .*\.rglob|subprocess|grep\(` как прокси ручных петель. Честная пометка: must_not по коду REPL — прокси, семантику гарантирует только real-model-прогон из п.3.
5. **Ловушка фикстуры** (правило ST-*): в ST-002 цепочка main→…→41 размазана по ≥3 файлам и имя промежуточной функции встречается ≥2 раз (trap против grep-угадывания). Проверить, что это так в фикстуре; если grep решает задачу — фикстура не держит.
6. **Стоимость:** `prime-agent prompt --json` до/после — дельта static-префикса (ожидаемо +2…+2.5k симв.). Одноразово, в том же PR; описания меняются редко, кэш-инвалидация однократная.

Ограничение честности: предлагаемая структура триггеров взята из корпусных текстов gitnexus (какие формулировки существуют), но каузальная связь «триггер → выбор» в корпусе не измерялась — её измеряет п.3.

---

# Приложение 1. ПОЛНЫЕ ТЕКСТЫ описаний gitnexus (17 тулов)

Источник: `mcp.describe_tool("gitnexus", <tool>)`, снято 2026-10-19 в этой сессии.
Сырой JSON: `/tmp/roadmap_audit/raw_tool_descs/gitnexus.json`.

## gitnexus / list_repos

```
List indexed repositories available to GitNexus (paginated).

Returns a page of repositories — each with name, path, indexed date, last commit, and stats — plus a "pagination" object: { total, limit, offset, returned, hasMore, nextOffset }.

PAGINATION: Results are paginated so a large registry is not truncated by MCP/LLM token limits. "limit" sets the page size (default 50, max 200; values above the max are rejected, not capped). "offset" selects the start. To enumerate EVERY repository: when pagination.hasMore is true, call list_repos again with offset set to pagination.nextOffset, and repeat until hasMore is false. Repositories are returned in a stable order, so paging never skips or duplicates an entry while the registry is unchanged.

WHEN TO USE: First step when multiple repos are indexed, or to discover available repos.
AFTER THIS: READ gitnexus://repo/{name}/context for the repo you want to work with.

When multiple repos are indexed, repo-scoped read-only tools use the configured
MCP default or the registered path containing the GitNexus process cwd, unless
cwd has crossed into an unindexed nested Git checkout. If neither applies,
specify the "repo" parameter explicitly.
```

## gitnexus / query

```
Query the code knowledge graph for execution flows related to a concept.
Returns processes (call chains) ranked by relevance, each with its symbols and file locations.

WHEN TO USE: Understanding how code works together. Use this when you need execution flows and relationships, not just file matches. Complements grep/IDE search.
AFTER THIS: Use context() on a specific symbol for 360-degree view (callers, callees, categorized refs).

Returns results grouped by process (execution flow):
- processes: ranked execution flows with relevance priority
- process_symbols: all symbols in those flows with file locations and module (functional area)
- definitions: standalone types/interfaces not in any process

Hybrid ranking: BM25 keyword + semantic vector search, ranked by Reciprocal Rank Fusion.

GROUP MODE: set "repo" to "@<groupName>" to search all member repos in that group (merged via RRF), or "@<groupName>/<groupRepoPath>" to run against a single member (same path keys as in group.yaml). If you use "@<groupName>" only, the member repo defaults to the lexicographically first key in group.yaml "repos". Prefer resources for contracts/status (see migration from legacy group_* tools).

SERVICE: optional monorepo path prefix (POSIX-style, case-sensitive segments). When "repo" starts with "@", only processes whose symbols fall under that prefix are included. For a normal indexed repo name (no leading @), this field is currently ignored by the server.
```

## gitnexus / cypher

```
Execute Cypher query against the code knowledge graph.

WHEN TO USE: Complex structural queries that search/explore can't answer. READ gitnexus://repo/{name}/schema first for the full schema.
AFTER THIS: Use context() on result symbols for deeper context.

SCHEMA:
- Nodes: File, Folder, Function, Class, Interface, Method, CodeElement, Community, Process, Route, Tool, Destination
- Multi-language nodes (use backticks): `Struct`, `Enum`, `Trait`, `Impl`, etc.
- All edges via single CodeRelation table with 'type' property
- Edge types: CONTAINS, DEFINES, IMPORTS, CALLS, EXTENDS, IMPLEMENTS, HAS_METHOD, HAS_PROPERTY, ACCESSES, METHOD_OVERRIDES, OVERRIDES, METHOD_IMPLEMENTS, MEMBER_OF, STEP_IN_PROCESS, HANDLES_ROUTE, PUBLISHES_TO, CONSUMES_FROM, FETCHES, HANDLES_TOOL, ENTRY_POINT_OF, WRAPS, QUERIES, INJECTS, CONDITIONAL_ON, DECLARES, ADVISED_BY, CFG, REACHING_DEF, TAINTED, SANITIZES, TAINT_PATH, CDG, POST_DOMINATE — CFG, REACHING_DEF, TAINTED, SANITIZES, TAINT_PATH, CDG, POST_DOMINATE are populated ONLY on indexes built with `gitnexus analyze --pdg` (zero rows on a default index); OVERRIDES is a legacy alias — rows are written as METHOD_OVERRIDES
- Edge properties: type (STRING), confidence (DOUBLE), reason (STRING), step (INT32)

EXAMPLES:
• Find callers of a function:
  MATCH (a)-[:CodeRelation {type: 'CALLS'}]->(b:Function {name: "validateUser"}) RETURN a.name, a.filePath

• Find community members:
  MATCH (f)-[:CodeRelation {type: 'MEMBER_OF'}]->(c:Community) WHERE c.heuristicLabel = "Auth" RETURN f.name

• Trace a process:
  MATCH (s)-[r:CodeRelation {type: 'STEP_IN_PROCESS'}]->(p:Process) WHERE p.heuristicLabel = "UserLogin" RETURN s.name, r.step ORDER BY r.step

• Find all methods of a class:
  MATCH (c:Class {name: "UserService"})-[r:CodeRelation {type: 'HAS_METHOD'}]->(m:Method) RETURN m.name, m.parameterCount, m.returnType

• Find all properties of a class:
  MATCH (c:Class {name: "User"})-[r:CodeRelation {type: 'HAS_PROPERTY'}]->(p:Property) RETURN p.name, p.declaredType

• Find all writers of a field:
  MATCH (f:Function)-[r:CodeRelation {type: 'ACCESSES', reason: 'write'}]->(p:Property) WHERE p.name = "address" RETURN f.name, f.filePath

• Find method overrides (MRO resolution):
  MATCH (winner:Method)-[r:CodeRelation {type: 'METHOD_OVERRIDES'}]->(loser:Method) RETURN winner.name, winner.filePath, loser.filePath, r.reason

• Find DI-injected providers (provider Classes or synthetic factory declarations):
  MATCH (c:Class {name: 'OrderService'})-[r:CodeRelation]->(provider) WHERE r.type = 'INJECTS' RETURN provider.name, r.reason

• Detect diamond inheritance:
  MATCH (d:Class)-[:CodeRelation {type: 'EXTENDS'}]->(b1), (d)-[:CodeRelation {type: 'EXTENDS'}]->(b2), (b1)-[:CodeRelation {type: 'EXTENDS'}]->(a), (b2)-[:CodeRelation {type: 'EXTENDS'}]->(a) WHERE b1 <> b2 RETURN d.name, b1.name, b2.name, a.name

OUTPUT: Returns { markdown, row_count } — results formatted as a Markdown table for easy reading.

TIPS:
- All relationships use single CodeRelation table — filter with {type: 'CALLS'} etc.
- Community = auto-detected functional area (Leiden algorithm). Properties: heuristicLabel, cohesion, symbolCount, keywords, description, enrichedBy
- Process = execution flow trace from entry point to terminal. Properties: heuristicLabel, processType, stepCount, communities, entryPointId, terminalId
- Use heuristicLabel (not label) for human-readable community/process names
- PDG layers (only when indexed with `--pdg`): BasicBlock nodes + CFG / CDG (control dependence, branch sense 'T'|'F' in reason) / REACHING_DEF (def→use, variable in reason) edges, all BasicBlock→BasicBlock. Prefer the `pdg_query` tool — it anchors + bounds these for you (raw `[:CDG*]`/`[:REACHING_DEF*]` path scans are unindexed and unbounded).
```

## gitnexus / context

```
360-degree view of a single code symbol.
Shows categorized incoming/outgoing references (calls, imports, extends, implements, methods, properties, overrides), process participation, and file location.

WHEN TO USE: After query() to understand a specific symbol in depth. When you need to know all callers, callees, and what execution flows a symbol participates in.
AFTER THIS: Use impact() if planning changes, or READ gitnexus://repo/{name}/process/{processName} for full execution trace.

Handles disambiguation: if multiple symbols share the same name, returns ranked candidates (each with a relevance score) for you to pick from. Use uid for zero-ambiguity lookup, or narrow the search with file_path and/or kind hints. The ambiguous response carries totalCandidates — the TRUE match count, not candidates[].length — plus candidatesTruncated:true and a "(showing M)" suffix on message when candidates[] is the shorter window.

NOTE: ACCESSES edges (field read/write tracking) are included in context results with reason 'read' or 'write'. CALLS edges resolve through field access chains and method-call chains (e.g., user.address.getCity().save() produces CALLS edges at each step).

COMPLETENESS OF incoming: alongside symbol/incoming/outgoing the result carries the same epistemic envelope impact() returns:
- epistemic: 'exact' | 'lower-bound' — 'lower-bound' means incoming is a FLOOR: either the walk provably missed callers, or a probe that would have established completeness could not run. Do not read it as proof that an omitted caller exists — read boundaries for which of the two it is.
- boundaries: string[] — one plain-language sentence per reason. Prose for humans; branch on causes instead.
- causes: { scopeExtractionFiles, receiverTyping, dispatchBoundary, externalBoundary, undecidedSatisfaction, callableValueReferences } — machine-readable WHY. Every field counts MISSING THINGS, never sentences:
  - causes.scopeExtractionFiles (unit: files) > 0 — scope extraction still failed after the fallback pass, so scope-resolution edges from those files are absent. A value of 0 does not prove completeness when epistemic is 'lower-bound' because an older or unverified index has no measured file count. Re-run `gitnexus analyze --force`; if the reason persists, inspect the extraction warnings.
  - causes.receiverTyping (unit: call sites) > 0 — RESOLVER GAP: the analyzer dropped that many call sites on this name because it could not type the receiver, so they are missing from incoming. Do not read an absent caller as proof none exists.
  - causes.externalBoundary (unit: call sites) > 0 — the calls left the indexed program (System.out.println, fetch(...)). NOT a defect: no in-graph node could have been reached. An epistemic:'exact' result can carry this.
  - causes.dispatchBoundary (unit: symbols) > 0 — DI or interface dispatch: that many symbols sit on or beyond a boundary static analysis cannot cross. Irreducible. A symbol count, not a site count — per-site multiplicity is not retained for these edges — so compare its magnitude with receiverTyping, not its exact value. A framework runtime-proxy boundary can make epistemic lower-bound while this value remains 0 because endpoint metadata proves the gap but cannot count omitted symbols.
  - causes.undecidedSatisfaction (unit: unjudged interface/type pairs) > 0 — the analyzer could not decide whether a type satisfies an interface, so no IMPLEMENTS edge exists and no dispatch boundary was left for the walk to notice. Usually fixable by making the missing dependency available to analysis.
  - causes.callableValueReferences (unit: symbols) > 0 — that many symbols name this callable as a VALUE instead of calling it (a Zig registration table or const initialiser, a JS/TS object-literal property value). A bare callback argument in JS/TS is not captured today and is not counted, so a 0 does not rule that shape out; nor does it, on an index built before the language emitted these captures — re-analyze first. The reference is in the graph as a USES edge; the call made THROUGH the value is not, because it is dispatched later from wherever the value was stored. incoming.calls is therefore a floor. Follow the USES edges to find the registration, then the code that reads it. It is 0 when the analyzer DID synthesize the dispatch through a registered property key. That exclusion is per SYMBOL, not per registration: a target with BOTH a followed registration and an unfollowed escape reads 0 here, so a 0 means 'no unfollowed registration was proven', not 'this symbol escapes nowhere'. A 0 alongside epistemic 'lower-bound' can also mean the probe itself could not run — read boundaries for which.

REQUIRES RE-INDEX: causes.scopeExtractionFiles, causes.receiverTyping, causes.externalBoundary, causes.undecidedSatisfaction, and framework runtime-proxy boundary detection depend on index-time metadata that only a current analyzer writes. Against an older index the metadata can be absent, which is indistinguishable from "nothing was dropped" unless the schema probe detects the stale index — re-run `gitnexus analyze` before trusting a zero or an apparently exact result.

GROUP MODE: set "repo" to "@<groupName>" to run context in each member repo (aggregated list), or "@<groupName>/<groupRepoPath>" for one member. If you use "@<groupName>" only, the member defaults to the lexicographically first key in group.yaml "repos".

SERVICE: optional monorepo path prefix (case-sensitive path segments). When "repo" starts with "@", prefix-matches resolved symbol file paths; when a hit is outside the prefix, that member returns an empty payload for the symbol. Ignored for a normal indexed repo name.
```

## gitnexus / detect_changes

```
Analyze uncommitted git changes and find affected execution flows.
Maps git diff hunks to indexed symbols, then traces which processes are impacted.

WHEN TO USE: Before committing — to understand what your changes affect. Pre-commit review, PR preparation.
AFTER THIS: Review affected processes. Use context() on high-risk symbols. READ gitnexus://repo/{name}/process/{name} for full traces.

GIT WORKTREE SUPPORT: GitNexus automatically detects when the MCP server was launched from inside a linked git worktree and runs git diff against that worktree — no extra parameters needed in the common case. Pass "worktree" explicitly only when the server was started from a different directory than the worktree you are editing (e.g., the server runs from the canonical root but your changes are in a linked worktree at a different path).

Returns: changed symbols, affected processes, and a risk summary.
- partial: true — a step failed and was swallowed, so the result is incomplete and risk_level is "unknown" instead of a ranked level. Two causes, with different blast radii: the symbol query (or an unparseable diff) degrades everything — changed_symbols, both counts, and the processes derived from them — while a failed process lookup degrades only affected_processes and the risk read off it, leaving the changed-symbol counts sound. changed_count:0 with partial:true is NOT a clean pre-commit check; re-run before treating the diff as safe.
- truncated: true — the changed_symbols LISTING was capped for this response. summary.changed_count counts every symbol the run observed: the true total normally, a LOWER BOUND when partial:true. Compare it with the array length rather than trusting the array.
```

## gitnexus / check

```
Run read-only structural checks against the indexed graph.

Currently detects directed cycles between File nodes connected by IMPORTS edges, counting only
edges that force a module-initialization order — a deferred import (`import()`, or one written
inside a function body) and a TypeScript `import type` are excluded, because neither can make the
modules impossible to initialize.

READ `enumeration` BEFORE `cycleCount`:
- `enumeration: 'complete'` — every elementary cycle is listed; `cycleCount` is their number.
- `enumeration: 'component-representatives'` — the full enumeration exceeded a safety limit, so
  `cycles` holds ONE representative per circular component, `truncated` is true, and
  `cycleCount` is **null**. Do not compare `cycleCount` numerically here: `null > 0` is false,
  so a caller keying on it alone concludes "clean" on exactly the most tangled repositories. Use
  `status === 'cycles_found'`.

`componentCount` (independent circular components) is present in both modes and is the number to
act on and to trend: cutting one import can remove thousands of elementary cycles at once, so
`cycleCount` swings wildly for small changes while `componentCount` stays stable.

A graph too large to analyze at all returns `{ error, truncated: true }` with no `status`.
```

## gitnexus / rename

```
Multi-file coordinated rename using the knowledge graph + text search.
Finds all references via graph (high confidence) and regex text search (lower confidence). Preview by default.

WHEN TO USE: Renaming a function, class, method, or variable across the codebase. Safer than find-and-replace.
AFTER THIS: Run detect_changes() to verify no unexpected side effects.

Each edit is tagged with confidence:
- "graph": found via knowledge graph relationships (high confidence, safe to accept)
- "text_search": found via regex text search (lower confidence, review carefully)

Handles disambiguation via context()'s payload verbatim: an ambiguous symbol_name returns status "ambiguous" with ranked candidates and totalCandidates — the TRUE match count, not candidates[].length — plus candidatesTruncated:true and a "(showing M)" suffix on message when candidates[] is the shorter window. Re-call with symbol_uid.
```

## gitnexus / impact

```
Analyze the blast radius of changing a code symbol.
Returns affected symbols grouped by depth, plus risk assessment, affected execution flows, and affected modules.

MODE (opt-in): "callgraph" (default) walks symbol→symbol edges (CALLS/IMPORTS/EXTENDS/IMPLEMENTS) — inter-procedural, the established comparator/default behavior. "pdg" requires an index built with `gitnexus analyze --pdg` and returns one unified PDG-facing result: statement-level control/data dependence from the persisted PDG plus inter-procedural symbol reach. The explicit interprocedural surface is interproceduralByDepth/pdgInterprocedural; byDepth remains the compatibility symbol bucket. pdg remains incompatible with crossDepth and @group targets; relationTypes/minConfidence filter the inter-symbol reach.

STATEMENT-ANCHORED PDG SLICE: with mode:'pdg', pass "line" (1-based source line within the target symbol) to seed the dependence slice on the statement at that line and return what depends on it in affectedStatements (line + text). Inter-procedural symbols are still reported through interproceduralByDepth/pdgInterprocedural and the compatibility byDepth bucket. Without "line", pdg returns whole-symbol inter-procedural reach plus local whole-symbol PDG diagnostics.

PDG OUTPUT CONTRACT: every mode:'pdg' result (success, empty, degraded, or error) carries pdgResultVersion:3 — a stable discriminator for external consumers that bumps on any breaking change to the PDG result shape (distinct from the DB schema version). Successful PDG results include mode:'pdg', a full target envelope (id/name/type/filePath), affectedStatements, affectedStatementCount, interproceduralByDepth/pdgInterprocedural for cross-function reach, compatibility byDepth/byDepthCounts, risk:'UNKNOWN', and a note describing the unified contract. Degraded PDG results (no-layer, sub-layer-missing, unknown) keep mode:'pdg', pdgResultVersion:3, target metadata when the target resolves, risk:'UNKNOWN', note/remediation, and empty byDepth parity fields — never a false-safe zero. If depth and limit both bound the slice, truncatedByReasons reports both causes while truncatedBy remains scalar. Return-value-ascent coverage is published structurally at pdgEvidence.ascent — present iff the inter-procedural descent ran, including on an empty slice — with referencesScanned (DISTINCT callees scanned for a CALL_SUMMARY: a distinct-id tally, not a call-site count — two call sites to the same callee count once), returnFlowFound (whether the ascent fired anywhere in the slice), undecodableSummaryCount, examinedComplete (whether that scan covered every callee the index recorded a resolved id for on the visited blocks), incompleteReasons ('traversal-truncated' | 'callee-list-capped' | 'callee-ids-unrecorded'), and callSummaryLayerPresent. Read callSummaryLayerPresent FIRST: false ⇒ a pre-CALL_SUMMARY index, so {referencesScanned:N>0, returnFlowFound:false} is self-consistent and says nothing about the callees — the scan ran, but no layer existed in which a return-flow could be recorded (remedy: re-run gitnexus analyze --pdg). Branch on those fields; the note narrates the same facts in prose for humans and is not a stable contract.

WHEN TO USE: Before making code changes — especially refactoring, renaming, or modifying shared code. Shows what would break.
AFTER THIS: Review d=1 items (WILL BREAK). Use context() on high-risk symbols.

Output includes:
- risk: LOW / MEDIUM / HIGH / CRITICAL / UNKNOWN. This is the HIGH/CRITICAL edit-gate field. File targets lack process/community membership, so their risk is not directly comparable with symbol risk; use riskSharedAxes to compare the direct/total axes common to both. Group-mode (`repo: "@…"`) results lift the same fields to the top-level envelope. The web Graph-RAG impact tool expands File targets to in-file symbols before enrichment, so process/cluster axes remain comparable there. An upstream walk that resolved ZERO callers reports UNKNOWN, never LOW, and carries riskNote: "safe to change" is a claim about callers and there were none to reason about, so the symbol is either genuinely unused OR reached only through a reference class the index does not record (plain-object property access, a bare-identifier read of a module-scope const). Confirm with a text search before acting on it. Downstream walks are unaffected — an empty downstream result reports resolved callees, not safety.
- riskSharedAxes: single-repo risk computed only from direct and total impact. Group mode then applies the cross-repo crossing overlay to that local value. Suitable for comparing File and symbol targets within the same mode. Never substitute it for `risk` when deciding whether to warn before edits.
- riskScale: { comparableAcrossKinds, unusedAxes } — names process/module axes that were structurally unavailable, skipped, budget-exhausted (`IMPACT_MAX_CHUNKS=0`), truncated (sampled a subset of impacted symbols), or failed at query time. Failed-query and truncated-sample counts are lower bounds: known HIGH/CRITICAL warnings survive, otherwise risk is UNKNOWN. Group impact copies this metadata from the local leg.
- riskNote: string — present only when risk is UNKNOWN; states why the verdict is withheld.
- summary: direct callers, processes affected, modules affected
- affected_processes: which execution flows break and at which step
- affected_modules: which functional areas are hit (direct vs indirect; classification-unavailable when that secondary query fails)
- byDepth: affected symbols grouped by traversal depth (paginated by limit/offset; omitted when summaryOnly:true — use byDepthCounts for totals per depth, pagination object when truncated). Each item includes a processes:[{id,label,processType,step}] field listing the execution flows that symbol participates in. Empty when the symbol has no process membership. Can ALSO be empty when partial:true is set — either the process-aggregation pass hit its cap before detecting affected processes, or per-symbol enrichment was capped on a very large page. When partial:true, do NOT treat processes:[] as proof of no participation; cross-check the top-level affected_processes list. An item carries staticGated:true only when the edge that reached it is provably unreachable at compile time from the indexed source (today: Zig calls inside an 'if (CONST_FALSE)' body or the else of 'if (CONST_TRUE)'); the field is absent when the edge is live or the language does not model it. Traversal and risk do NOT filter or rank on it: it is metadata for the caller to weigh.
- epistemic: 'exact' | 'lower-bound' — whether impactedCount is the whole story. 'lower-bound' means the count is a FLOOR: either the walk provably missed callers, or a probe that would have established completeness could not run (a failed callable-value-reference query says so in boundaries). It is not itself proof that an omitted caller exists — branch on causes and read boundaries. Absent only on skipped probes (ambiguous-candidate lists, group fan-out).
- boundaries: string[] — one plain-language sentence per reason the count is short. Prose for humans; branch on causes instead.
- causes: { scopeExtractionFiles, receiverTyping, dispatchBoundary, externalBoundary, undecidedSatisfaction, callableValueReferences } — the machine-readable split of WHY, so an agent gating its own edits can tell a fixable analyzer gap from an irreducible one. Every field counts MISSING THINGS, never sentences:
  - causes.scopeExtractionFiles (unit: files) > 0 — scope extraction still failed after the fallback pass, so scope-resolution edges from those files are absent. A value of 0 does not prove completeness when epistemic is 'lower-bound' because an older or unverified index has no measured file count. Re-run `gitnexus analyze --force`; if the reason persists, inspect the extraction warnings.
  - causes.receiverTyping (unit: call sites) > 0 — the RESOLVER GAP signal: the analyzer dropped that many call sites because it could not establish the receiver's type (unresolved constructor, factory, chained expression). Those callers are absent from byDepth. Treat the result as incomplete: grep the symbol name before deleting or renaming.
  - causes.externalBoundary (unit: call sites) > 0 — those calls left the indexed program (System.out.println, fetch(...), os.environ.*). NOT a defect and NOT a reason the count is short: there is no in-graph node any edge could have reached. An epistemic:'exact' result can carry this.
  - causes.dispatchBoundary (unit: symbols) > 0 — DI or interface dispatch: that many symbols sit on or beyond a boundary a static walk cannot cross. Irreducible. A symbol count, not a site count — per-site multiplicity is not retained for these edges — so compare its magnitude with receiverTyping, not its exact value. A framework runtime-proxy boundary can make epistemic lower-bound while this value remains 0 because endpoint metadata proves the gap but cannot count omitted symbols.

  - causes.undecidedSatisfaction (unit: unjudged interface/type pairs) > 0 — the analyzer could not DECIDE whether a type satisfies an interface (a type in a required signature named a package it could not resolve), so no IMPLEMENTS edge exists and no dispatch boundary was left for the walk to notice. Distinct from every cause above, which count decided facts that could not be attributed; this one counts questions never answered. It is the only cause that shortens a result WITHOUT leaving a trace in the graph, so an unhedged zero on a symbol reached only through such an interface would otherwise read as 'nobody calls this'. Usually fixable: it most often means a dependency is missing from the analyzed tree.
  - causes.callableValueReferences (unit: symbols) > 0 — that many symbols name this callable as a VALUE rather than calling it: 'bridge.accessor(Element.getNamespaceUri, ...)' and 'pub const h = onReset;' in Zig, '{ onClick: handler }' in JS/TS. Those are the shapes actually captured today — a bare callback argument in JS/TS ('qsort'-style, 'setTimeout(tick)') is NOT one of them and is not counted, so a 0 here does not rule that shape out. The registration IS modelled (a USES edge); the invocation through the stored value is NOT, because it happens later via a struct field, a registry lookup or comptime reflection. So impactedCount is a floor and a LOW risk verdict on such a symbol is a floor too. Unlike dispatchBoundary this is often reducible — it usually means the language provider does not yet follow that store/load — but until it is, do NOT read an empty or small caller set as 'safe to change'. It is an exact count, not a capped sample. It is 0 when the analyzer DID synthesize the dispatch through a registered property key, and epistemic stays 'exact' on that account. That exclusion is symbol-level, not edge-level — the graph does not record which registration produced which synthesized call — so a symbol with a mix of followed and unfollowed registrations also reads 0: treat a 0 as 'no unfollowed registration was proven', not as proof the value escapes nowhere. A 0 alongside epistemic 'lower-bound' can instead mean the probe could not run at all, so read boundaries to tell those apart. Read from the graph, so it needs no index-time metadata BEYOND the edges being there: an index built by an analyzer that did not yet emit this language's value-ref captures has none, and reports 0. Re-analyze before reading a 0 as measured.

REQUIRES RE-INDEX: causes.scopeExtractionFiles, causes.receiverTyping, causes.externalBoundary, causes.undecidedSatisfaction, and framework runtime-proxy boundary detection depend on index-time metadata that only a current analyzer writes. Against an older index the metadata can be absent, which is indistinguishable from "nothing was dropped" unless the schema probe detects the stale index — re-run `gitnexus analyze` before trusting a zero or an apparently exact result.

Depth groups:
- d=1: WILL BREAK (direct callers/importers)
- d=2: LIKELY AFFECTED (indirect)
- d=3: MAY NEED TESTING (transitive)

TIP: For hub symbols (base error classes, shared utilities) with many direct callers, use summaryOnly: true first to see counts and risk, then drill into specific depths with limit/offset. maxDepth alone does not bound output size when most dependents are at depth 1. limit and offset apply independently to each depth level, not to the total result set — use byDepthCounts to see totals per depth.

TIP: Default traversal uses CALLS/IMPORTS/EXTENDS/IMPLEMENTS. For class members, include HAS_METHOD and HAS_PROPERTY in relationTypes. For field access analysis, include ACCESSES in relationTypes.

Handles disambiguation: when multiple symbols share the target name, returns ranked candidates (each with a relevance score) instead of silently picking one. Use target_uid for zero-ambiguity lookup, or narrow with file_path and/or kind hints. totalCandidates is the TRUE match count — it reported the capped resolver window before #2787, so it can now exceed candidates.length; candidatesTruncated:true and a "(showing M of N)" suffix on message mark the shorter window.

EdgeType: CALLS, IMPORTS, EXTENDS, IMPLEMENTS, HAS_METHOD, HAS_PROPERTY, METHOD_OVERRIDES, METHOD_IMPLEMENTS, ACCESSES
Confidence: 1.0 = certain, <0.8 = fuzzy match

GROUP MODE: set "repo" to "@<groupName>" for cross-repo impact anchored at the default member (lexicographically first key in group.yaml "repos"), or "@<groupName>/<groupRepoPath>" to choose the member (same path keys as in group.yaml). Phase-1 walk runs in that member; cross-boundary fan-out uses the group bridge. A cross entry with fanout_status:"not_attempted" proves the declared repository boundary, but its far endpoint has no graph symbol; do not interpret empty by_depth or affected_processes on that entry as a completed zero-impact walk. The fan-out attempts at most 50 neighbour crossings, strongest-confidence first. Any short answer carries truncated:true, truncatedRepos, riskEpistemic:"lower-bound" AND a truncationReason — dropping a crossing can only move risk DOWN, so treat that risk as a floor, never as a verdict. truncated:true does NOT always mean the fan-out ran out of room, so branch on truncationReason: the remedy differs. 'timeout' (the fan-out's wall-clock budget expired) and 'partial' (a neighbour crossing, or the local walk, was cut short) are runtime limits — the same query can return more on a retry or with a larger timeoutMs. 'incomplete-sync' is structural: the group bridge was built by a sync that could not say which repos it read, or that could not read an in-scope repo, so those repos' contracts are absent from EVERY query against this bridge, and truncatedRepos names them even when ZERO crossings to them were attempted. Retrying returns the same floor — run group_sync (`gitnexus group sync`) and query again. 'suppressed-stage' is also structural but has a DIFFERENT remedy: the sync was asked to skip a matching stage (`--exact-only` / exactOnly), so cross-links that stage would have found are absent BY REQUEST. Re-running the sync unchanged returns the same floor — re-run it WITHOUT that flag. Do not report a repo as broken for this reason; nothing failed to read.

SERVICE: optional monorepo path prefix (case-sensitive path segments). When "repo" starts with "@", scopes the local impact walk and cross-repo symbol paths to files under that prefix; ignored for a normal indexed repo name.
```

## gitnexus / explain

```
Explain persisted taint findings recorded by `gitnexus analyze --pdg`: intra-procedural source→sink data flows (TAINTED edges, statement-level hops) AND cross-function flows (TAINT_PATH edges, function-level hops, marked `interprocedural: true`).

Each finding carries the sink category (command-injection, code-injection, path-traversal, sql-injection, xss) and the ordered hop path. Intra-procedural findings carry source/sink lines and the variable on each hop; interprocedural findings carry the source and sink FUNCTION names and the chain of functions the taint crossed (decoded from the persisted path encoding).

WHEN TO USE: Security review — "what taint findings exist in this repo / file / function?". Requires the repo to be indexed with `gitnexus analyze --pdg`; without that layer the tool returns a clear "no taint layer" note, not an error.

ANCHORLESS (no "target"): enumerates all persisted findings for the repo — bounded ("limit", deterministic order), with "totalFindings" and a "truncated" flag.
ANCHORED ("target" = file path or symbol/function name): full hop detail for that anchor. A file-ish target (contains "/" or an extension) filters by file; a symbol name resolves like context() — ambiguous names return ranked candidates plus totalCandidates (the TRUE match count, not candidates[].length), candidatesTruncated:true and a "(showing M)" suffix on message when candidates[] is the shorter window; unknown names return not-found. Symbol anchoring is line-range granular for intra-procedural findings; cross-function findings match when the symbol is the source OR sink function.

CONTRACT CAVEATS (absent flows are NOT proof of safety):
- Cross-function flows ARE modeled (#2084 M4): a source flowing through helper functions into a sink is found, via summary composition over the call graph (context-insensitive — return/call-site merging is accepted).
- Cross-function matching is by callee NAME (context-insensitive): when one caller invokes two distinct same-named callees, a flow into one over-attributes to both — a cross-function finding does not prove the taint reached every same-named function (sound over-report, never a missed flow).
- Closure/callback flows are invisible in both directions (e.g. arr.forEach(() => sink(y))) — the largest false-negative class.
- Property/field flows are not tracked (obj.x = taint; sink(obj.y) has no chain).
- Guard-style sanitizers (if (isValid(x))) and implicit/control-dependence flows are not modeled.
- CommonJS aliasing is partially modeled (require('<literal>') joins resolve; dynamic requires do not).
- Exception-path over-approximation can produce false-positive noise.

Findings are deliberately NOT part of impact()'s traversal or the web schema — explain is the dedicated taint consumer. SANITIZES (kill) edges are queryable via cypher.
```

## gitnexus / pdg_query

```
Query the persisted Program Dependence Graph recorded by `gitnexus analyze --pdg` — control dependence (CDG) and data dependence (REACHING_DEF) at basic-block granularity. The control/data analog of `explain` (which is the taint consumer).

MODES:
- `controls` — "under what condition does X run?". Returns, for the anchored function, each control-dependence edge: the controlling predicate block, the dependent block, and the branch sense ('T' = the predicate's true/taken arm, 'F' = its false/fall-through arm). An edge into an early return/throw block is flagged `guard: true` (subsumes the #559 guard heuristic); the branch sense of a guard depends on its predicate — `if (!ok) return;` rides the 'T' arm — so don't filter guards by a fixed label.
- `flows` — "where does variable Y flow?". Returns REACHING_DEF def→use edges for the anchored function; pass `variable` to filter to one binding.

WHEN TO USE: comprehension ("what guards this statement?"), data-flow tracing within a function, guard-clause discovery. Requires `gitnexus analyze --pdg`; without that layer the tool returns a clear "no PDG layer" note, not an error.

ANCHORING (required): `target` is a file path or a symbol/function name (resolved like context()). PDG queries are ALWAYS anchored — there is no whole-repo enumeration (an unanchored basic-block path scan is unbounded; LadybugDB has no rel-property index). A symbol target is line-range granular; an ambiguous name returns ranked candidates plus totalCandidates (the TRUE match count, not candidates[].length), candidatesTruncated:true and a "(showing M)" suffix on message when candidates[] is the shorter window; unknown returns not-found.

CONTRACT CAVEATS:
- CDG labels are binary 'T'/'F' in M5/M6; per-case `switch` arm conditions are not yet distinguished (every case dispatch is 'T').
- Granularity is basic-block, reconstructed to the function via the BasicBlock id + line span (no Function→BasicBlock edge); deeply same-line-packed functions may anchor coarsely.
- Control/data dependence is intra-procedural (per function). Cross-function flow is taint's domain (`explain`).
- These edges are deliberately NOT part of impact()'s traversal — `pdg_query` is the dedicated consumer; raw edges are also queryable via `cypher`.
```

## gitnexus / route_map

```
Show API route mappings: which components/hooks fetch which API endpoints, and which handler files serve them.

WHEN TO USE: Understanding API consumption patterns, finding orphaned routes. For pre-change analysis, prefer `api_impact` which combines this data with mismatch detection and risk assessment.
AFTER THIS: Use impact() on specific route handlers to see full blast radius.

Returns: route nodes with their handlers, middleware wrapper chains (e.g., withAuth, withRateLimit), and consumers. Each route object includes its "method" (the HTTP verb, "*" for method-agnostic routes, or null for method-less routes) and "runtimeEvidence". Runtime evidence is authoritative only when runtimeEvidence.confirmed is true; source records provenance, including conflicts.
```

## gitnexus / tool_map

```
Show MCP/RPC tool definitions: which tools are defined, where they're handled, and their descriptions.

WHEN TO USE: Understanding tool APIs, finding tool implementations, impact analysis for tool changes.

Returns: tool nodes with their handler files and descriptions.
```

## gitnexus / shape_check

```
Check response shapes for API routes against their consumers' property accesses.

WHEN TO USE: Detecting mismatches between what an API route returns and what consumers expect. Finding shape drift. For pre-change analysis, prefer `api_impact` which combines this data with mismatch detection and risk assessment.
REQUIRES: Route nodes with responseKeys (extracted from .json({...}) calls during indexing).

Returns routes that have both detected response keys AND consumers. Shows top-level keys each endpoint returns (e.g., data, pagination, error) and what keys each consumer accesses. Reports MISMATCH status when a consumer accesses keys not present in the route's response shape. Each route object includes its "method" (the HTTP verb, "*" for method-agnostic routes, or null for method-less routes) and "runtimeEvidence". Runtime evidence is authoritative only when runtimeEvidence.confirmed is true.
```

## gitnexus / api_impact

```
Pre-change impact report for an API route handler.

WHEN TO USE: BEFORE modifying any API route handler. Shows what consumers depend on, what response fields they access, what middleware protects the route, and what execution flows it triggers. Requires at least "route" or "file" parameter.

Risk levels: LOW (0-3 consumers), MEDIUM (4-9 or any mismatches), HIGH (10+ consumers or mismatches with 4+ consumers). Mismatches with confidence "low" indicate the consumer file fetches multiple routes — property attribution is approximate.

Response shape is keyed on how many routes match, not on the data: exactly one match returns a single route object; two or more return { routes: [...], total: N }. The same URL can expose multiple HTTP verbs (e.g. GET and POST /api/orders are distinct routes that share the URL), so a bare-URL lookup may return the wrapped form — every route object carries its own "method" so verbs are distinguishable. Pass "method" to narrow to one verb; the single-object shape is returned only when exactly one route remains after filtering — a substring route/file match spanning several URLs can still return the wrapped form. A URL/file that exists but has no route for the given verb returns an error. Each route's "method" is the literal "*" for method-agnostic routes (e.g. Django function views), which match any "method" selector, or null for method-less routes (filesystem, Laravel resource), which never match a selector. Every route also carries "runtimeEvidence"; treat it as authoritative only when runtimeEvidence.confirmed is true. Combines route_map, shape_check, and impact data.
```

## gitnexus / group_list

```
List all configured repository groups, or return details for one group (repos, manifest links).

WHEN TO USE: Discover groups before group_sync. Optional "name" returns a single group's config.
```

## gitnexus / group_sync

```
Rebuild the Contract Registry (contracts.json) for a group: extract contracts (HTTP, gRPC, Thrift, topics, includes), apply manifest links, then cross-link by exact contract-id match followed by wildcard service match.

WHEN TO USE: After changing group.yaml or re-indexing member repos.

READ THE RESULT: `missingRepos` are configured repos with no entry in the registry (index them, or drop them from group.yaml); `unreadableRepos` ARE registered but this sync could not extract from them — the index would not open (version skew, lock, corruption), or an extractor failed partway — so NONE of their contracts are in this sync and a following group_impact / group_contracts is a lower bound, not a verdict. `degradedLinks` is the count of persisted cross-links whose provider endpoint has no resolved graph symbol (`degraded: true`); re-analyze the provider so handlers resolve. `failedRepos` is `{ repo, reason }[]` for per-repo extraction throws — each also appears in `unreadableRepos`; `repo` is that group path (e.g. app/backend), not the registry display name. `warnings` are operator-facing run notes (e.g. bridge.lbug write failed after contracts.json was written); `[]` means none this run. `registryOutcome` says what happened to the file, and the three values a call here can return each need a different response: 'written' — this run's contracts replaced contracts.json; 'preserved' — nothing could be read, so contracts.json was rewritten keeping the previous sync's contracts and cross-links verbatim and refreshing only `missingRepos`/`unreadableRepos` to describe THIS run (the file changed, the contracts in it did not, and they are as old as the last sync that succeeded); 'superseded' — nothing could be read, and another sync replaced contracts.json while this one waited for the group lock; that file was left untouched and this run's lists were NOT recorded, because they describe an older group state than what is on disk (so the registry is fresher than this response's diagnostics, not staler); 'no-prior-registry' — nothing could be read AND there was no previous contracts.json to carry forward, so none was written and this group has no contract registry on disk. Only 'no-prior-registry' means there is nothing to read: after it, group_contracts / group_impact have no registry at all rather than a stale one, so fix the repos above and re-run before trusting either. `suppressedMatchStages` names matching stages this sync was ASKED to skip, with the same three states as the repo lists: ABSENT means a registry written before the field existed, `[]` means this sync suppressed nothing, and a populated list means the cross-link set is a lower bound BY REQUEST — a later group_impact / group_contracts on it reports truncationReason 'suppressed-stage'.

PARAMETERS ARE VALIDATED: `exactOnly` must be a real boolean — the string "false" is rejected, not coerced to true. The retired `skipEmbeddings` and `allowStale` parameters are refused by name; drop them from the call.
```

## gitnexus / trace

```
Find the shortest directed path between two symbols over call and class-member edges.

WHEN TO USE: Debugging "how does A reach B?" — answers in one call what would take 3-8 manual context/impact hops. Shows the exact chain with file:line positions plus a per-hop edge type and confidence.

Traverses CALLS edges plus HAS_METHOD (class → member) edges, so a trace can descend from a class into its methods. Each hop's edge type is reported in edges[], so call hops and containment hops remain distinguishable.

Returns: ordered hops with file:line, and an aligned edges[] of edge type + confidence. When no path exists, reports the furthest reachable node so you know where the chain breaks (and truncated: true if a traversal cap was hit first).

Handles disambiguation: an ambiguous from/to name returns status "ambiguous" with role ("from" or "to"), ranked candidates and totalCandidates — the TRUE match count, not candidates[].length — plus candidatesTruncated:true and a "(showing M)" suffix on message when candidates[] is the shorter window. Re-call with from_uid/to_uid.

CROSS-REPO (experimental): pass repo as "@groupName" to trace across repositories in a group. When from/to live in different member repos, the trace stitches the two repo-local segments across a single ContractLink boundary (e.g. an HTTP consumer→provider link), clamped to one crossing. The result adds crossings[] (the bridged contract with matchType/confidence), tags each hop with its member repo, and a notes[] channel for degraded states. The boundary hop is reported with edge type CONTRACT_LINK. Pass pdg:true to also attach the intra-procedural data-flow (REACHING_DEF) for boundary-adjacent segments when those repos were indexed with --pdg; absent a PDG layer it degrades to call-level hops with a note.

DESTINATION TRACE (cross-repo): for an "@groupName" trace, OMIT to/to_uid/to_file to trace 'from' to wherever its outgoing HTTP call lands. The result ends at the provider endpoint (reported by route + file even when the handler is an anonymous function with no nameable symbol). This is the way to follow a client call to a backend handler you cannot name.
```


---

# Приложение 2. ПОЛНЫЕ ТЕКСТЫ описаний codebase-memory-mcp (17 тулов)

Источник: `mcp.describe_tool("codebase-memory-mcp", <tool>)`, снято 2026-10-19.
Сырой JSON: `/tmp/roadmap_audit/raw_tool_descs/codebase_memory.json`.
Для сравнения: даже в этой «минималистской» системе смысл живёт в описаниях
**параметров** (см. `query_graph.max_rows`, `trace_path.include_evidence`), поэтому ниже
после каждого описания приведены и описания параметров (сокращённо до 120 симв.).

## codebase-memory / index_repository

```
Index a repository. full/moderate add semantics; fast omits them; cross-repo-intelligence links services. Reports coverage gaps.
```

Параметры:
- `repo_path`: Repository path
- `mode`: full: all+semantic; moderate: filtered+semantic; fast: filtered only; cross-repo-intelligence: link services.
- `target_projects`: Cross-repo targets; ["*"] means all.
- `name`: Name override; Non-ASCII bytes are encoded; unsafe characters normalized.
- `persistence`: Write .codebase-memory/graph.db.zst.

## codebase-memory / search_graph

```
Find symbols via BM25 query, regex name/qn filters, or semantic_query. Rows keep qn/file/lines and in/out over CALLS/USAGE/CALL_REFERENCE/INHERITS/IMPLEMENTS.
```

Параметры:
- `semantic_query`: Not with query.

## codebase-memory / query_graph

```
Read-only Cypher for multi-hop, aggregation, complexity, or cross-service analysis. Default: 200 visible rows with exact/lower-bound totals and truncation; continue safely with next_cursor. graph=missed is a file tree of flagged coverage gaps; absence is not proof of completeness. Use get_graph_schema(diagnostics=full) for properties.
```

Параметры:
- `query`: Cypher query
- `graph`: code graph (default) or missed coverage-gap file tree.
- `max_rows`: Visible rows (default 200; max 99998); 0 uses the legacy maximum. Evaluation is unchanged.
- `offset`: Live compatibility paging; cannot be combined with cursor.
- `cursor`: Snapshot continuation; keep query/project/graph. Format, budget, and max_rows may change.
- `max_output_tokens`: Sizing hint; hard ceiling is 4 UTF-8 bytes/token. Rows stay whole.
- `format`: tree may use local @N+suffix prefix refs; json uses direct strings.

## codebase-memory / trace_path

```
Trace callers/callees, data flow, or cross-service paths. Defaults exclude tests and resolver evidence. Rows keep qn/hop with explicit totals, relations, and continuations.
```

Параметры:
- `limit`: Rows/page; gte flags the 5000-node engine ceiling.
- `max_output_tokens`: Sizing hint; hard ceiling is 4 UTF-8 bytes/token. Evidence/args yield before nearest graph rows.
- `cursor`: Pass next/next_cursor with traversal args unchanged; budget may increase.
- `mode`: calls, argument-aware data_flow, or service edges.
- `parameter_name`: data_flow parameter filter.
- `risk_labels`: Add hop-risk labels.
- `format`: tree chooses smaller complete direct/grouped output; json uses stable grouped tables.
- `include_evidence`: Add resolver class and confidence.

## codebase-memory / get_code_snippet

```
Read a search_graph symbol. auto bounds source and outlines large containers; full restores up to 500 lines. Source/outline pages continue; coverage_note marks gaps.
```

Параметры:
- `qualified_name`: search_graph qn, or short name.
- `source_mode`: auto outlines 200+ line containers; full returns source; outline lists members.
- `max_output_tokens`: auto/outline sizing hint (default 2500), hard ceiling 4 UTF-8 bytes/token. Lines stay whole; explicit full is uncapped unless set.

## codebase-memory / get_file_outline

```
Declaration outline of one exact repository-relative file: optional exact label filter, source order, exact total/offset/limit paging; file/folder/container nodes excluded.
```

Параметры:
- `file_path`: Exact repository-relative file path
- `labels`: Optional exact node-label filter

## codebase-memory / get_graph_schema

```
Get node-label and edge-type counts. diagnostics=full also lists queryable properties.
```

Параметры:

## codebase-memory / compare_graphs

```
Compare two indexed snapshots: deterministic target-only additions and base-only removals of stable node/edge identities; each set capped by limit and a 512 KiB budget with exact totals and truncation reasons.
```

Параметры:

## codebase-memory / get_architecture

```
Compact counts, languages, packages, entry points. Request structure, dependencies, routes, hotspots, boundaries, layers, clusters, cycles, or file_tree; path scopes a directory.
```

Параметры:
- `path`: Directory prefix (for example apps/hoa).
- `aspects`: all=everything; overview=compact except file_tree; omitted=languages/packages/entry_points; cycles is opt-in.

## codebase-memory / search_code

```
Graph-ranked text search: compact symbols, full bounded source, or file paths.
```

Параметры:
- `debug`: Add scope_ms/scan_ms/enrich_ms phase timings.
- `raw_content_offset`: Raw-line byte offset; omit for match-centered preview.

## codebase-memory / list_projects

```
List projects with stable paging. Identity is lean; stats adds graph sizes.
```

Параметры:
- `detail`: stats adds node/edge/database-size counts.
- `include_details`: Alias for detail=stats: include branch, node/edge counts and database size. Slower.
- `metadata_only`: Compatibility: omit counts, size, and branch.

## codebase-memory / delete_project

```
Delete a project from the index
```

Параметры:

## codebase-memory / index_status

```
Project readiness, counts, root, and coverage gaps. diagnostics adds coverage rows; verbose adds Git paths. Best-effort only; verify cited paths with check_index_coverage.
```

Параметры:
- `verbose`: Add worktree/shadow Git paths for index-location debugging.
- `diagnostics`: Coverage rows: counts, five samples, or up to 500.

## codebase-memory / check_index_coverage

```
Best-effort exact-path/scope coverage and freshness, paged separately. full diagnostics adds raw detail. Clean is not proof of completeness.
```

Параметры:

## codebase-memory / detect_changes

```
Map a Git diff to files and impact. Page with snapshot cursors.
```

Параметры:
- `direction`: inbound=callers; outbound=dependencies; both=union.
- `max_output_tokens`: Sizing hint; hard ceiling is 4 UTF-8 bytes/token.

## codebase-memory / manage_adr

```
Outline an ADR by default; get reads it; update replaces it
```

Параметры:
- `mode`: outline pages headings; get reads; update replaces the whole document; set_sections rewrites only the named sections and leaves every other byte untouched (an identical repeated write is byte-identica…
- `content`: Whole document for update
- `section_updates`: set_sections: section name -> new body. Any heading name works; names match exactly, including case.

## codebase-memory / ingest_traces

```
Validate and count traces; graph edge creation is not implemented
```

Параметры:
