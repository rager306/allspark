# Codex tool calling — разбор для prime-agent

Дата: 2026-10-18. Codex CLI 0.163.0 (`/usr/lib/node_modules/@openai/codex`), локальные
`~/.codex/*`, GitHub `openai/codex@main`, docs `developers.openai.com`.

**Метод.** Помимо чтения исходников, собрана экспериментальная установка: локальный
mock-провайдер (`/tmp/cx/mock3.py`) отдаёт Codex по SSE заранее заданные tool-calls, а
Codex исполняет их по-настоящему и присылает обратно `custom_tool_call_output` —
т.е. наблюдалось **ровно то, что видит модель**, а не догадки. Плюс `codex debug prompt-input`
и `codex debug models` для точных model-facing спеков, `codex --codex-run-as-apply-patch`
для прямой проверки парсера патчей.

Все URL ниже — `https://github.com/openai/codex/blob/main/` + путь.

---

## 0. Главное открытие: как Codex вообще описывает тулы

В 0.163 модель по умолчанию получает **не 20 JSON-схем, а один freeform-JS-тул `exec`**
(`type: "custom"`, 14 027 символов описания), внутри которого все остальные тулы доступны
как `await tools.exec_command(...)`. Это «Code Mode».

- Файл: `codex-rs/core/src/tools/handlers/shell_spec.rs`, `.../apply_patch_spec.rs`
- Наблюдённый payload: `/tmp/cx/reqs2/req_1.json` → `input[0].type == "additional_tools"`,
  `input[0].tools[0].name == "functions"`, `tools[0].tools[0].name == "exec"`.
- Размеры (замерено): `exec` desc **14 027** симв.; `spawn_agent` **1 829**;
  весь блок `additional_tools` **25 347** симв. Developer-промпт **21 420** симв.
  Прочие тулы (`apply_patch`, `exec_command`, `write_stdin`, `view_image`, …) в этом
  режиме **вообще не приходят как отдельные схемы** — только строкой TypeScript-объявления
  внутри описания `exec`:

```ts
declare const tools: { exec_command(args: {
  // Shell command to execute.
  cmd: string;
  ...
}): Promise<{ chunk_id?: string; exit_code?: number; ... }>; };
```

Вывод: Codex экономит верхнеуровневые схемы, но платит за это **одним огромным описанием**,
в котором поля документированы комментариями `//` над каждым свойством. Это ровно тот приём,
который нам стоит рассмотреть для `lsp` (22 действия) — сейчас у нас одна плоская строка
описания (`crates/pa-lsp/src/tool.rs:958`) и большая JSON-схема.

---

## 1. Формат ответа (по пунктам задания)

### ID: tool-desc-tiny-apply-patch
- **ИСТОЧНИК**: `codex-rs/core/src/tools/handlers/apply_patch_spec.rs:17-26`; `codex-rs/core/assets/tools/apply_patch.lark` (20 строк)
- **СИМПТОМ / ПРИЁМ**: описание `apply_patch` — **одно предложение**: «The `apply_patch` tool can be used to edit files. This is a FREEFORM tool, so do not wrap the patch in JSON.» Вся спецификация формата — **Lark-грамматика** (20 строк), а не проза. Никаких «when to use / when NOT to use», примеров и ограничений в описании нет.
- **НАШ АНАЛОГ**: `crates/pa-edit/prompts/apply_patch.md` (2 112 симв.) + `grammars/apply_patch.lark` — у нас **уже тот же приём**, но текст в 10× богаче (есть примеры, правило «не дублировать context», запрет absolute path).
- **ЧТО ДЕЛАТЬ**: перенять не нужно — мы уже лучше. Зафиксировать регрессионным тестом, что `description(EditMode::ApplyPatch)` не пуст и грамматика парсится: вход `pa_edit::description(ApplyPatch)` → ожидание `contains("*** Begin Patch")` и `grammar(ApplyPatch).is_some()`.
- **СЕРЬЁЗНОСТЬ**: low

### ID: tool-desc-structure-codex
- **ИСТОЧНИК**: `/tmp/cx/reqs2/req_1.json` (`input[0].tools[0].tools[0].description`); `codex-rs/core/src/tools/handlers/tool_search_spec.rs:88-91`
- **СИМПТОМ / ПРИЁМ**: описания строятся по шаблону: (1) одна строка «что делает», (2) буллиты «- Evaluates… - All nested tools… - Runs raw JS…», (3) глобальные хелперы с сигнатурами, (4) TS-типы, (5) по каждому тулу — `### name`, проза + `declare const tools`. Ограничения даются **в теле описания параметра**, а не отдельным блоком: `// Wait before yielding output. Defaults to 10000 ms; effective range is 250-30000 ms.`
- **НАШ АНАЛОГ**: `lsp.description()` — одна длинная строка на 500+ символов без структуры; `graph`, `edit`, `jev` аналогично.
- **ЧТО ДЕЛАТЬ**: перенять. Для `lsp` сгенерировать описание из таблицы действий: на каждое из 22 действий — 1 строка «что делает» + 1 строка «когда не использовать» + пример аргументов. Регрессионный тест: вход — `lsp` tool spec; ожидание — `description.len() > 2000 && description.matches("when NOT").count() >= 10`.
- **СЕРЬЁЗНОСТЬ**: high

### ID: tool-desc-deferred-discovery
- **ИСТОЧНИК**: `codex-rs/tools/src/tool_executor.rs:49-97` (`enum ToolExposure`), `codex-rs/core/src/tools/handlers/tool_search_spec.rs`
- **СИМПТОМ / ПРИЁМ**: шесть уровней экспозиции тула: `Direct`, `Deferred`, `DeferredModelOnly`, `DirectModelOnly`, `CodeModeOnly`, `Hidden`. `Deferred` = тул не в первичном списке, но находится через `tool_search` (BM25 по метаданным). Плюс `ALL_TOOLS` внутри `exec` — модель может отфильтровать список доступного в рантайме.
- **НАШ АНАЛОГ**: `crates/pa-core/src/tool_profile/disclosure.rs` — у нас есть disclosure-профили, но, судя по `registry.rs`, без поиска по метаданным.
- **ЧТО ДЕЛАТЬ**: перенять идею «deferred + поиск» для 22 действий `lsp`: держать в промпте 5–6 ходовых (`find_symbol`, `references`, `diagnostics`, `rename`, `replace_symbol_body`, `safe_delete_symbol`), остальные 16 — через `lsp(action="list")` / поиск. Регрессионный тест: вход — профиль `lsp`; ожидание — `direct_actions.len() <= 6 && deferred_actions.contains("code_action")`.
- **СЕРЬЁЗНОСТЬ**: high

### ID: apply-patch-atomic-vs-cli
- **ИСТОЧНИК**: `codex-rs/core/src/tools/handlers/apply_patch.rs:186-215` (`verify_apply_patch_args` → потом `execute_verified_patch`); `codex-rs/apply-patch/src/invocation.rs:33-90`; эксперимент `/tmp/cx/sc_atomic`
- **СИМПТОМ / ПРИЁМ**: **две разные семантики в одном продукте.** Model-facing `apply_patch` сначала **верифицирует все хунки** (`try_verify_apply_patch_args` собирает `HashMap<path, change>` и падает до записи), поэтому атомарен: патч `Update a.txt` + `Update b.txt` с битым контекстом во втором → `a.txt` не изменён. Standalone CLI `codex --codex-run-as-apply-patch` пишет **по мере обхода хунков**: тот же патч → `a.txt` уже изменён, `b.txt` нет, exit-код ошибки. Проверено: `a.txt: 'A1CHANGED\nA2\n'` при провале.
- **НАШ АНАЛОГ**: `crates/pa-edit/src/modes/apply_patch.rs:265` — `ATOMICITY_NOTICE = "No files were modified — sections apply atomically."`, стадия → writer. Мы атомарны.
- **ЧТО ДЕЛАТЬ**: регрессионный тест на нашу атомарность (её легко сломать при рефакторинге): вход — патч из 2 файлов, второй с несуществующим контекстом; ожидание — `Err`, и **оба** файла байт-в-байт равны исходным.
- **СЕРЬЁЗНОСТЬ**: critical

### ID: apply-patch-duplicate-path
- **ИСТОЧНИК**: `codex-rs/apply-patch/src/invocation.rs:62-69`; issue [#50549](https://github.com/openai/codex/issues/50549); эксперимент `/tmp/cx/sc_dup`
- **СИМПТОМ / ПРИЁМ**: два `*** Update File: f.txt` в одном патче → `apply_patch verification failed: invalid patch: multiple operations target /tmp/cx/f.txt`. Документировано **только в коде**, в model-facing описании этого нет — отсюда 83 проваленных вызова за 30 дней у автора issue. Отдельно: `*** Add File` на **существующий** файл Codex **молча перезаписывает** (`exists.txt` → `new`), хотя грамматически это «create».
- **НАШ АНАЛОГ**: `crates/pa-edit/src/modes/apply_patch.rs:312-325` — у нас `Cannot create {path}: file already exists. Use *** Update File to modify…`. Дубликат пути: строка 308 разрешает `delete`+`add` одного пути (осознанно).
- **ЧТО ДЕЛАТЬ**: у нас Add-on-existing уже закрыт лучше Codex. Добавить тест на дубликат `Update` одного пути: вход — два `Update File` с разными хунками; ожидание — детерминированная ошибка с именем пути и подсказкой «объедините в один блок». Если сейчас проходит — это дыра.
- **СЕРЬЁЗНОСТЬ**: high

### ID: apply-patch-error-no-hint
- **ИСТОЧНИК**: эксперименты `/tmp/cx/sc_patch`, `/tmp/cx/sc_patch2`; `codex-rs/apply-patch/src/file_update.rs:150-155`
- **СИМПТОМ / ПРИЁМ**: все ошибки — плоский текст без структуры и без подсказки. Наблюдено дословно: `apply_patch verification failed: Failed to find expected lines in /tmp/cx/f.txt:\nalpha`; `Failed to find context '-1,3 +1,4 @@' in /tmp/cx/f.txt`; `Failed to read file to update /tmp/cx/ap/missing.txt: No such file or directory (os error 2)`; `patch rejected: writing is blocked by read-only sandbox; rejected by user approval settings`. Ни номера строки, ни «что делать». Модель после этого обычно повторяет тот же патч.
- **НАШ АНАЛОГ**: `EditError::Match(String)` / `Apply(String)` — тоже текст, но у нас есть `line: Option<u32>` в `Parse`.
- **ЧТО ДЕЛАТЬ**: перенять обратное — добавить в `EditError::Match` структурированные поля `{path, hunk_index, searched_from_line, closest_candidate}` и печатать подсказку. Тест: вход — патч с отсутствующим контекстом; ожидание — сообщение содержит `path`, номер хунка и слово `re-read`.
- **СЕРЬЁЗНОСТЬ**: high

### ID: apply-patch-numeric-hunk-header
- **ИСТОЧНИК**: issue [#52889](https://github.com/openai/codex/issues/52889); воспроизведено: `Failed to find context '-1,3 +1,4 @@' in f.txt`
- **СИМПТОМ / ПРИЁМ**: стандартный unified-diff заголовок `@@ -1,3 +1,4 @@` не поддерживается: парсер трактует `-1,3 +1,4 @@` как **литеральный контекст** и ищет эту строку в файле. Модели, обученные на git-diff, генерируют такой заголовок постоянно. Плюс конфликт с локальными инструкциями (у пользователя было требование «hunk headers must contain ranges») → цикл «парсер-ошибка → запрещённый вариант → approval».
- **НАШ АНАЛОГ**: `grammars/apply_patch.lark` — `change_context: ("@@" | "@@ " /(.+)/) LF`, т.е. `@@ -1,3 +1,4 @@` попадёт в `/(.+)/` как заголовок и, скорее всего, будет проигнорирован (не как контекст).
- **ЧТО ДЕЛАТЬ**: (а) принять и игнорировать числовой диапазон; (б) если не принимаем — вернуть **явную** ошибку `unsupported hunk header format: '@@ -1,3 +1,4 @@'; use bare '@@' or '@@ <symbol>'`. Тест: вход — `@@ -1,3 +1,4 @@` + валидные строки; ожидание — либо успех, либо ошибка со словом `unsupported` (никогда `Failed to find context '-1,3...'`).
- **СЕРЬЁЗНОСТЬ**: high

### ID: apply-patch-hunk-order-constraint
- **ИСТОЧНИК**: issue [#50549](https://github.com/openai/codex/issues/50549); `codex-rs/apply-patch/src/file_update.rs:60-140` (`line_index` только растёт)
- **СИМПТОМ / ПРИЁМ**: хунки матчатся **по исходному файлу в прямом порядке**, `line_index = start_idx + pattern.len()` не откатывается. Следствия (все воспроизведены): (A) обратный порядок → `Failed to find expected lines` для строки, которая **есть** в файле; (B) второй хунк не может матчить строку, вставленную первым. Это ограничение не описано модели.
- **НАШ АНАЛОГ**: `pa-edit` — stage/apply по файлу; нужно проверить, монотонен ли курсор.
- **ЧТО ДЕЛАТЬ**: тест-«ловушка»: вход — файл `alpha/middle/omega`, патч с хунками в обратном порядке; ожидание — понятная ошибка `hunks must be in file order (hunk 2 matched before hunk 1)` вместо `Failed to find`. И тест, что прямой порядок работает.
- **СЕРЬЁЗНОСТЬ**: medium

### ID: apply-patch-idempotency-missing
- **ИСТОЧНИК**: эксперимент `/tmp/cx/ap` (двойное применение); issue [#43714](https://github.com/openai/codex/issues/43714)
- **СИМПТОМ / ПРИЁМ**: повторное применение того же патча → `Failed to find expected lines … alpha`, т.е. **не идемпотентно и не распознаёт «уже применено»**. Модель в цикле ретраев получает ту же ошибку. Родственный баг: модель вообще уходит от `apply_patch` к python-скриптам правки файлов, и пользователю приходится настаивать.
- **НАШ АНАЛОГ**: `EditStore` (snapshots, no-op guard) — у нас есть no-op guard.
- **ЧТО ДЕЛАТЬ**: перенять диагностику «уже применено»: если `old_lines` не найдены, но `new_lines` уже стоят на этом месте → вернуть `already applied (no-op)` как **успех**, а не ошибку. Тест: вход — применить патч дважды; ожидание — второй вызов `Ok` с `no_op: true`, файл не изменён.
- **СЕРЬЁЗНОСТЬ**: high

### ID: apply-patch-silent-ambiguity
- **ИСТОЧНИК**: `codex-rs/apply-patch/src/seek_sequence.rs:33-56`; эксперимент `/tmp/cx/amb`
- **СИМПТОМ / ПРИЁМ**: при нескольких вхождениях контекста Codex **молча берёт первое** (`for i in search_start..` → первый матч). Файл `k/DUP/k2/DUP/k3`, патч `-DUP +UNIQ` → изменена **первая** DUP, ошибки нет. Модель может «попасть не туда» и не узнать об этом. Усугубляется 4-ступенчатым фаззи-матчем: exact → rstrip → trim → **Unicode-нормализация** (тире/кавычки/NBSP → ASCII).
- **НАШ АНАЛОГ**: `pa-edit/src/fuzzy.rs`, `modes/apply_patch.rs` (exact-match engine).
- **ЧТО ДЕЛАТЬ**: перенять обратное — при >1 кандидате возвращать ошибку `ambiguous context: N matches at lines [a, b]; add more context or use '@@ <symbol>'`. Тест: вход — файл с двумя одинаковыми блоками, патч с 1 строкой контекста; ожидание — `Err(ambiguous)` со списком строк.
- **СЕРЬЁЗНОСТЬ**: high

### ID: apply-patch-fuzzy-overreach
- **ИСТОЧНИК**: `codex-rs/apply-patch/src/seek_sequence.rs:70-120`; эксперимент `/tmp/cx/edge` (indentation)
- **СИМПТОМ / ПРИЁМ**: последний проход матча — `trim()` обеих сторон. Патч с `-  return 1` (2 пробела) применился к файлу с `    return 1` (4 пробела) → файл стал `  return 2`, **отступ сломан**, ошибки нет. Для Python/YAML это тихая порча кода.
- **НАШ АНАЛОГ**: `pa-edit/src/fuzzy.rs`, режим `sloppy` (осознанно мягкий).
- **ЧТО ДЕЛАТЬ**: тест-страж: вход — патч, где old-line отличается от файла **только ведущими пробелами**; ожидание — `Err` (или явный warning в результате), но **не** тихая замена с потерей отступа. Отдельно: fuzzy-допуск не должен применяться к языкам со значимым отступом.
- **СЕРЬЁЗНОСТЬ**: critical

### ID: exec-pty-session-model
- **ИСТОЧНИК**: `codex-rs/core/src/tools/handlers/shell_spec.rs:96-140`; `/tmp/cx/reqs2/req_1.json`
- **СИМПТОМ / ПРИЁМ**: один тул `exec_command` покрывает и «выполнить до конца», и «запустить в фоне». Решение принимает **сам тул по таймауту**, а не модель: `yield_time_ms` (default 10 000, диапазон 250–30 000). Если процесс не успел — возвращается `session_id`, и модель обязана поллить через `write_stdin`. `tty: bool` (default false) переключает PTY vs pipes. Отдельного «фонового» флага нет.
- **НАШ АНАЛОГ**: `python_repl` — основной инструмент; отдельного shell-тула с сессиями у нас нет.
- **ЧТО ДЕЛАТЬ**: перенять контракт для любого будущего exec-тула: **один** тул + `yield_time_ms` + `session_id` + `write_stdin`, без «background: true». Тест: вход — `sleep 5` с `yield_time_ms=300`; ожидание — ответ содержит `session_id`, `output == ""`, `exit_code` отсутствует.
- **СЕРЬЁЗНОСТЬ**: medium

### ID: exec-truncation-head-tail
- **ИСТОЧНИК**: эксперимент `/tmp/cx/sc_trunc2`; `codex-rs/core/src/unified_exec` (`UNIFIED_EXEC_OUTPUT_MAX_BYTES` = 1 MiB)
- **СИМПТОМ / ПРИЁМ**: вывод режется по **head + tail** с явной маркой в двух местах: шапка `Warning: truncated output (original token count: 27224)\nTotal output lines: 20000`, внутри — `42…17224 tokens truncated…7`. Модель видит и факт обрезки, и сколько потеряно. Параметры: `max_output_tokens` (default 10 000) на вызов.
- **НАШ АНАЛОГ**: `python_repl` — обрезка есть, но, судя по всему, без `original_token_count`.
- **ЧТО ДЕЛАТЬ**: перенять формат: в результате всегда `original_token_count` + маркер `…N tokens truncated…` в середине. Тест: вход — вывод 20 000 строк; ожидание — результат содержит `truncated`, `original_token_count > 0`, первые и последние строки на месте.
- **СЕРЬЁЗНОСТЬ**: medium

### ID: exec-truncation-silent-drop-bug
- **ИСТОЧНИК**: issue [#50041](https://github.com/openai/codex/issues/50041); `codex-rs/core/src/unified_exec/process.rs:131`, `async_watcher.rs:125-127`
- **СИМПТОМ / ПРИЁМ**: **баг**. `broadcast::channel(64)`; при `RecvError::Lagged(_)` ветка делает `continue`, теряя чанки. Итог: `aggregated_output` в событии **теряет сотни KB из середины без маркера** (замерено автором: 808 490 / 857 642 / 693 802 байт вместо 1 048 606). Модель не затронута, а **TUI/клиент — да**: пользователь видит неполный вывод как полный. Тест `unified_exec_formats_large_output_summary` флакует на медленных машинах.
- **НАШ АНАЛОГ**: любые `broadcast`-каналы стриминга (`pa-agent`, `pa-tui`).
- **ЧТО ДЕЛАТЬ**: регрессионный тест на **любой** наш broadcast-стрим: вход — продюсер пишет 1.3 MB, консьюмер отстаёт; ожидание — либо всё, либо явный `... N bytes omitted ...`; **никогда** тихая дыра. Плюс grep-правило: `Err(Lagged` без учёта `n` — баг.
- **СЕРЬЁЗНОСТЬ**: critical

### ID: exec-background-turn-end
- **ИСТОЧНИК**: issue [#14731](https://github.com/openai/codex/issues/14731); воспроизведено `/tmp/cx/sc_bg`
- **СИМПТОМ / ПРИЁМ**: **баг**. Тёрн завершается, пока `unified_exec`-процесс ещё жив: `ResponseEvent::Completed` проверяет только `has_pending_input()`, но не `process_store`. Воспроизведено: `sleep 8` с `yield_time_ms=300` → модель получила `session_id: 53596`, тут же сказала «done», **тёрн закрылся**, `DONE` так и не пришёл. Модель даже не получила предупреждение «у тебя висит фоновый процесс».
- **НАШ АНАЛОГ**: цикл тёрна в `pa-agent`; наши subagent'ы.
- **ЧТО ДЕЛАТЬ**: перенять фикс, а не баг: при завершении тёрна проверять «есть ли незакрытые session_id» и либо инжектить developer-сообщение `A background terminal started in this turn is still running (session_id=…). Poll it with write_stdin or acknowledge detachment.`, либо не закрывать тёрн. Тест: вход — сценарий выше; ожидание — тёрн **не** завершён, в контексте есть предупреждение с `session_id`.
- **СЕРЬЁЗНОСТЬ**: critical

### ID: exec-empty-poll-storm
- **ИСТОЧНИК**: `/root/.codex/sessions/2026/06/29/rollout-2026-06-29T12-53-46-*.jsonl` (реальная сессия, 1 857 строк)
- **СИМПТОМ / ПРИЁМ**: **баг в дикой природе.** Замерено по логу: 362 tool-call'а, из них **300 `write_stdin`**, из них **105 с `Original token count: 0`**, максимальная серия подряд — **152 идентичных вызова** (`{"session_id":17425,"chars":"","yield_time_ms":30000,"max_output_tokens":16000}`), 37 из них пустые. Суммарно 75 минут ожидания в одном тёрне. Пустой полл стоит модели ~114 символов и **не несёт информации**: `Chunk ID / Wall time: 30.0011 seconds / Process running with session ID 17425 / Original token count: 0 / Output:`. Codex никак не бэкоффит и не предупреждает.
- **НАШ АНАЛОГ**: любые wait/poll-циклы (subagent `wait`, LSP-таймауты).
- **ЧТО ДЕЛАТЬ**: (1) при `N` пустых поллов подряд возвращать **не** «пусто», а `no new output after N polls (~M min); consider a longer yield_time_ms or background`; (2) экспоненциальный бэкофф `yield_time_ms`; (3) в UI — счётчик. Тест: вход — 5 пустых `write_stdin` подряд; ожидание — 5-й ответ содержит `no new output` и рекомендованный `yield_time_ms > 30000`.
- **СЕРЬЁЗНОСТЬ**: high

### ID: exec-schema-strictness
- **ИСТОЧНИК**: `/tmp/cx/sc_schema`; `codex-rs/core/src/tools/handlers/unified_exec.rs:28-46` (`#[derive(Deserialize)]`, `#[serde(default)]`)
- **СИМПТОМ / ПРИЁМ**: `strict: false` + `additionalProperties: false` в схеме, но **лишние поля молча игнорируются**: `{cmd:"echo OK", bogus_field:1}` → успех. А вот отсутствующее/неверно типизированное поле даёт **точную** серриализационную ошибку: `failed to parse function arguments: missing field \`cmd\` at line 1 column 2`; `invalid type: integer \`123\`, expected a string at line 1 column 10`. Плюс расхождение: `timeout_ms` **есть** в Rust-структуре, но **нет** в model-facing схеме → модель про него не знает (и он не сработал: `timeout_ms:1500` для `sleep 20` → всё равно 10 с по `yield_time_ms`).
- **НАШ АНАЛОГ**: `pa-lsp/src/tool.rs:989` — `serde_json::from_value(params).map_err(|err| anyhow!("invalid lsp arguments: {err}"))`. Формат близкий, но без `line/column`.
- **ЧТО ДЕЛАТЬ**: (а) не иметь полей в структуре, которых нет в схеме (или помечать `#[serde(skip)]`); (б) сохранить в наших ошибках `line/column` от serde — это буквально подсказка модели, куда смотреть. Тест: вход — `lsp(action="find_symbol")` без `symbol`; ожидание — сообщение содержит `missing field` и `column`.
- **СЕРЬЁЗНОСТЬ**: medium

### ID: exec-timeout-not-in-schema
- **ИСТОЧНИК**: `codex-rs/core/src/tools/handlers/unified_exec.rs:39`; `shell_spec.rs:44-90`
- **СИМПТОМ / ПРИЁМ**: `timeout_ms` объявлен в `ExecCommandArgs`, но в `create_exec_command_tool*` его **нет**. Значит: (1) модель не может задать жёсткий таймаут; (2) единственный способ — `yield_time_ms`, который таймаутом **не является** (процесс продолжает жить). Это источник «вечных» фоновых процессов (#42717: прерывание тёрна оставляет процесс).
- **НАШ АНАЛОГ**: таймауты `lsp` (`timeout_secs`).
- **ЧТО ДЕЛАТЬ**: если добавляем exec-тул — `timeout_ms` обязан быть в схеме и обязан **убивать** процесс. Тест: вход — `sleep 60, timeout_ms=1000`; ожидание — `exit_code` = timeout-код и процесс отсутствует в `ps`.
- **СЕРЬЁЗНОСТЬ**: medium

### ID: errors-are-plain-text
- **ИСТОЧНИК**: `/tmp/cx/sc_patch2`, `/tmp/cx/sc_wrongtool`, `/tmp/cx/sc_stale`, `/tmp/cx/sc_deny2`
- **СИМПТОМ / ПРИЁМ**: все ошибки — **плоские строки**, без кода/поля/подсказки: `unsupported call: nonexistent_tool`; `write_stdin failed: Unknown process id 999999999`; `Script error:\nTypeError: tools.not_a_real_tool is not a function\n    at exec_main.mjs:1:23`; `patch rejected: writing is blocked by read-only sandbox; rejected by user approval settings`. Ни `error_code`, ни `retryable`, ни `hint`. Единственная структура — обёртка `Script failed / Wall time X / Output:` + текст.
- **НАШ АНАЛОГ**: `EditError` — enum с вариантами, но `Display` = плоский текст.
- **ЧТО ДЕЛАТЬ**: перенять обратное — отдавать модели **JSON** `{ok:false, code:"MATCH_NOT_FOUND", path, hunk, hint, retryable:true}` и **дублировать** одной строкой для совместимости. Тест: вход — ошибка матчинга; ожидание — результат парсится как JSON и содержит `code` + непустой `hint`.
- **СЕРЬЁЗНОСТЬ**: high

### ID: errors-model-visible-ok
- **ИСТОЧНИК**: `/tmp/cx/sc_wrongtool` → `req_2.json`
- **СИМПТОМ / ПРИЁМ**: **правильное поведение**, которое стоит зафиксировать: ошибка тула **не роняет тёрн** и **не теряется** — она приходит модели как обычный `function_call_output` с тем же `call_id`, и цикл продолжается:
```json
{"type":"function_call","name":"nonexistent_tool","call_id":"f1","arguments":"{}"},
{"type":"function_call_output","call_id":"f1","output":"unsupported call: nonexistent_tool"}
```
- **НАШ АНАЛОГ**: роутер тулов в `pa-agent`.
- **ЧТО ДЕЛАТЬ**: регрессионный тест: вход — вызов несуществующего действия `lsp(action="nope")`; ожидание — в следующем сообщении есть tool-result с тем же `call_id`, `is_error` (или текст), и агент продолжает, а не падает.
- **СЕРЬЁЗНОСТЬ**: high

### ID: parallel-opt-in-per-tool
- **ИСТОЧНИК**: `codex-rs/tools/src/tool_executor.rs:113-116`; `codex-rs/core/src/tools/parallel.rs:170-186`; `.../unified_exec/exec_command.rs` (`true`), `.../apply_patch.rs` (нет метода → `false`), `.../mcp.rs`
- **СИМПТОМ / ПРИЁМ**: параллельность — **opt-in на каждый тул**, по умолчанию `false`. Механика: `RwLock<()>`; `supports_parallel == true` → `lock.read()` (параллельно), иначе `lock.write()` (эксклюзивно). `exec_command` и `write_stdin` — `true`; `apply_patch` — **`false`** (мутации сериализуются); MCP — `true` только если сервер объявил `supports_parallel_tool_calls` **или** тул помечен `readOnlyHint`.
- **НАШ АНАЛОГ**: у нас 4 нативных тула; `edit` — мутирующий, `lsp` — смешанный (rename мутирует!), `graph` — индекс мутирует, `jev` — read-only.
- **ЧТО ДЕЛАТЬ**: перенять **точно**: `supports_parallel` — свойство **действия**, не тула. Для `lsp`: `find_symbol/references/diagnostics/hover` → parallel; `rename/replace_symbol_body/safe_delete_symbol` → exclusive. Тест: вход — батч из `lsp(references)` + `lsp(rename)`; ожидание — rename выполняется эксклюзивно (в логе нет пересечения по времени), а два `references` — параллельны.
- **СЕРЬЁЗНОСТЬ**: critical

### ID: parallel-same-file-conflict
- **ИСТОЧНИК**: эксперимент `/tmp/cx/sc_parpatch`
- **СИМПТОМ / ПРИЁМ**: два **параллельных** `apply_patch` на один файл (`one/two/three` → `ONE`, `THREE`) отработали оба, файл корректен (`ONE\ntwo\nTHREE`), оба вернули `{}`. Это **не** гарантия: `apply_patch` не parallel-safe, просто в code-mode вызовы внутри одного `exec` шли последовательно (`await`). Риск реален при `Promise.all` — там порядок недетерминирован, а проверка «файл ещё не изменён» делается до записи.
- **НАШ АНАЛОГ**: `pa-edit` + `EditWriter` (host-owned). Конфликт правок одного файла — прямой риск.
- **ЧТО ДЕЛАТЬ**: **не** полагаться на удачу: блокировка по пути файла (`path → Mutex`) на время verify+write. Тест: вход — `Promise.all([patch(A), patch(A)])` с конфликтующими хунками; ожидание — один успех + один `Err(stale: file changed since read)`; файл не повреждён.
- **СЕРЬЁЗНОСТЬ**: critical

### ID: tool-loop-unbounded
- **ИСТОЧНИК**: issue [#27759](https://github.com/openai/codex/issues/27759); реальный лог (152 подряд, см. `exec-empty-poll-storm`)
- **СИМПТОМ / ПРИЁМ**: **баг подтверждён в проде.** Codex **не ограничивает** повторяющиеся идентичные вызовы. Issue с детерминированным репро: 1 287 запросов к провайдеру, 826 255 повторов одного вызова, процесс убит по OOM. В моём локальном логе — 152 идентичных `write_stdin` подряд. Никакого «no progress» детектора для тулов нет (единственный похожий механизм — `update_goal(status="blocked")`, который требует **3** повторяющихся блокирующих тёрна, и он про goal'ы, а не про tool loop).
- **НАШ АНАЛОГ**: цикл агента в `pa-agent`.
- **ЧТО ДЕЛАТЬ**: перенять **фикс**, а не баг. Детектор: `hash(tool_name + canonical_args)` в скользящем окне; при `K` идентичных подряд (предлагаю 3) — инжектить developer-сообщение `You have called X with identical arguments N times. The result has not changed. Change approach or report the blocker.`; при `2K` — жёстко прерывать тёрн с диагностикой. Тест: вход — мок-модель, отдающая один и тот же `exec_command` 5 раз; ожидание — на 4-м вызове в контексте появляется предупреждение, на 6-м тёрн прерывается, всего вызовов ≤ 6 (не 826 255).
- **СЕРЬЁЗНОСТЬ**: critical

### ID: tool-loop-parse-error-no-abort
- **ИСТОЧНИК**: issue [#27760](https://github.com/openai/codex/issues/27760)
- **СИМПТОМ / ПРИЁМ**: **баг.** В параллельном ответе один вызов schema-invalid, соседний — валиден. Codex репортит ошибку парсинга первому, но **продолжает цикл и повторно исполняет валидного соседа**: 590 241 повтор каждого из двух вызовов, 1 126 побочных эффектов при ожидаемых ≥2.
- **НАШ АНАЛОГ**: батч-обработка tool-calls в `pa-agent`.
- **ЧТО ДЕЛАТЬ**: правило «один невалидный вызов в батче → весь батч не исполняется, ошибки возвращаются по всем `call_id`». Тест: вход — батч `[valid_exec, invalid_args_exec]`; ожидание — побочный эффект от valid **ровно один раз** (или ноль), оба `call_id` получили ответ, цикла нет.
- **СЕРЬЁЗНОСТЬ**: critical

### ID: wrong-tool-no-steering
- **ИСТОЧНИК**: issue [#43714](https://github.com/openai/codex/issues/43714); `codex-rs/models-manager/prompt.md`
- **СИМПТОМ / ПРИЁМ**: модель зовёт **не тот** тул (python-скрипт вместо `apply_patch`), и Codex **ничего не делает** — ни детектора, ни наказующего сообщения. Единственная защита — строчка в промпте: «Use the `apply_patch` tool to edit files (NEVER try `applypatch` or `apply-patch`, only `apply_patch`)» + «Do not waste tokens by re-reading files after calling `apply_patch`». Родственный баг [#2235](https://github.com/openai/codex/issues/2235): модель зовёт `apply_patch` **как shell-команду** → `apply_patch: command not found` (бинаря нет на PATH, это виртуальный тул).
- **НАШ АНАЛОГ**: у нас 4 нативных тула + `python_repl`; риск «модель пишет файл через `python_repl` вместо `edit`» — тот же.
- **ЧТО ДЕЛАТЬ**: (1) **детектор**: если `python_repl`-код содержит `open(...,'w')` / `Path.write_text` / `subprocess…sed -i` — вернуть предупреждение `use the edit tool for file modifications` (не блокировать); (2) при `2+` таких случаях — добавить в developer-сообщение напоминание. Тест: вход — `python_repl` с `open("x","w")`; ожидание — в результате есть предупреждение с именем `edit`.
- **СЕРЬЁЗНОСТЬ**: high

### ID: exec-heredoc-apply-patch-detection
- **ИСТОЧНИК**: issue [#2235](https://github.com/openai/codex/issues/2235); `codex-rs/apply-patch/src/invocation.rs:97-140` (`maybe_parse_apply_patch`)
- **СИМПТОМ / ПРИЁМ**: Codex **специально** распознаёт `apply_patch <<'EOF'` внутри `bash -lc` (tree-sitter + строгий query: только `cd X && apply_patch <<EOF`, один позиционный аргумент, без `;`/`&& echo`). Всё, что не подошло, исполняется как настоящая shell-команда → `command not found`. Т.е. они латают «модель зовёт виртуальный тул через shell» эвристикой парсера.
- **НАШ АНАЛОГ**: не применимо напрямую (у нас нет `apply_patch`-бинаря), но **принцип** важен: любая «виртуальная» команда должна либо существовать на PATH, либо детектироваться.
- **ЧТО ДЕЛАТЬ**: не копировать эвристику. Если делаем exec-тул — выставить на PATH **настоящий** шим `apply_patch` (как `codex --codex-run-as-apply-patch`), чтобы `apply_patch <<EOF` работал буквально. Тест: вход — `bash -lc "apply_patch <<'EOF' … EOF"`; ожидание — патч применён, exit 0 (не `command not found`).
- **СЕРЬЁЗНОСТЬ**: medium

### ID: tool-output-context-blowup
- **ИСТОЧНИК**: issues [#16664](https://github.com/openai/codex/issues/16664), [#40897](https://github.com/openai/codex/issues/40897)
- **СИМПТОМ / ПРИЁМ**: **баг.** Один `function_call_output` с `Original token count: 948372` / `Total output lines: 23781` уходит в контекст целиком. В [#40897](https://github.com/openai/codex/issues/40897): 51 tool-call, 7.24 M input-токенов, вход на вызов вырос 21 k → 194 k, 5-часовой лимит ушёл на 49 % за 16 минут. Причина — модель просит `max_output_tokens: 60000`, получает обрезанный результат, идёт искать заново, обрезки накапливаются.
- **НАШ АНАЛОГ**: результаты `lsp`/`graph` на больших репозиториях.
- **ЧТО ДЕЛАТЬ**: (1) **жёсткий** потолок на tool-result в контексте (напр. 8 k токенов) независимо от запроса модели; (2) не отдавать `max_output_tokens` на откуп модели (или кэпить); (3) при обрезке — **указывать, как сузить запрос**. Тест: вход — `lsp(action="references")` в файле с 10 k ссылок; ожидание — результат ≤ N токенов, содержит `truncated` и `narrow the query`.
- **СЕРЬЁЗНОСТЬ**: critical

### ID: call-output-pairing
- **ИСТОЧНИК**: issue [#46193](https://github.com/openai/codex/issues/46193); [#42088](https://github.com/openai/codex/issues/42088)
- **СИМПТОМ / ПРИЁМ**: **баг класса «заклинивание».** Инвариант «у каждого `function_call` есть `function_call_output`» проверяется **только** при перестройке истории (`normalize_history` → `ensure_call_outputs_present`), но **не** на границе отправки (`build_responses_request`). Первый запрос тёрна вообще идёт мимо снапшота (`initial_input.take()`). Итог: строгий upstream отвечает `No tool output found for tool call X`, тёрн остаётся в истории, и **каждый следующий** тёрн воспроизводит ошибку — вечный клинч. Родственно: `function_call_output` без `call_id` (42088).
- **НАШ АНАЛОГ**: сборка сообщений в `pa-ai`/`pa-agent`.
- **ЧТО ДЕЛАТЬ**: **барьер на отправке**: перед каждым запросом валидировать `set(call_ids(calls)) == set(call_ids(outputs))`, при расхождении — синтезировать `{"status":"aborted"}` для висящих и **выкидывать** осиротевшие outputs. Тест: вход — история с `function_call` без output; ожидание — запрос уходит, в нём есть синтетический output `aborted`, upstream не 400.
- **СЕРЬЁЗНОСТЬ**: critical

### ID: exec-cwd-uri-bug
- **ИСТОЧНИК**: issue [#51175](https://github.com/openai/codex/issues/51175)
- **СИМПТОМ / ПРИЁМ**: Linux 0.160.x: `unified_exec` передаёт cwd как `file://` URI → **каждая** команда падает с `Failed to create unified exec process: No such file or directory (os error 2)`. Классика «внутреннее представление пути просочилось в системный вызов».
- **НАШ АНАЛОГ**: `pa-edit/src/path_policy.rs` (`UrlResolution`, `PathUri`), `pa-lsp` пути.
- **ЧТО ДЕЛАТЬ**: тест-страж на границе «наш PathUri/URL → OS path»: вход — путь, пришедший как `file:///tmp/x`; ожидание — конвертируется в `/tmp/x` до любого `spawn`/`open`; и обратный тест: OS-путь никогда не уходит в API как URL.
- **СЕРЬЁЗНОСТЬ**: high

### ID: exec-interrupt-leaks-process
- **ИСТОЧНИК**: issues [#42717](https://github.com/openai/codex/issues/42717), [#30802](https://github.com/openai/codex/issues/30802), [#35193](https://github.com/openai/codex/issues/35193)
- **СИМПТОМ / ПРИЁМ**: прерывание тёрна **не убивает** in-flight процесс: он остаётся жить (WSL/headless Chrome — «survive timeout and keep consuming CPU»). `timeout` не освобождает агента, если живёт дочерний процесс.
- **НАШ АНАЛОГ**: `python_repl` (долгоживущий), LSP-серверы (`pa-lsp/src/manager.rs`), `graph` reindex.
- **ЧТО ДЕЛАТЬ**: на отмену — kill **process group** (`kill(-pgid)`), а не только лидера; дождаться reap. Тест: вход — запустить `bash -c 'sleep 300 & sleep 300'`, отменить тёрн; ожидание — в `ps` нет ни одного из процессов через 1 с.
- **СЕРЬЁЗНОСТЬ**: high

### ID: exec-approval-escalation-params
- **ИСТОЧНИК**: `/tmp/cx/reqs2/req_1.json` → `input[1]` (`<permissions instructions>`); `shell_spec.rs:190-260`
- **СИМПТОМ / ПРИЁМ**: эскалация прав — **параметры того же вызова**, а не отдельный тул: `sandbox_permissions: "use_default" | "require_escalated"`, `justification` (вопрос пользователю), `prefix_rule` (напр. `["git","pull"]` — «запомни это разрешение»). Плюс явный **чёрный список** префиксов: «do not request `["python3"]`, `["python","-"]`… NEVER provide prefix_rule for destructive commands like rm. NEVER … if your command uses a heredoc». И правило «сначала попробуй в песочнице, при sandbox-подобной ошибке — **перезапусти** с `require_escalated`, не спрашивая заранее».
- **НАШ АНАЛОГ**: у нас нет модели approvals в тулах (кроме plan-mode).
- **ЧТО ДЕЛАТЬ**: если вводим approvals — перенять: (1) параметры в том же вызове, (2) `prefix_rule` с чёрным списком, (3) «retry with escalation» вместо «ask first». Тест: вход — `edit` в read-only зоне; ожидание — результат содержит `sandbox_permissions`-подсказку и `justification`-шаблон, а не просто отказ.
- **СЕРЬЁЗНОСТЬ**: medium

### ID: prompt-drift-unknown-model
- **ИСТОЧНИК**: issue [#44529](https://github.com/openai/codex/issues/44529)
- **СИМПТОМ / ПРИЁМ**: **баг.** Fallback-промпт для неизвестной модели содержит `{"command":["apply_patch","…"]}` (legacy argv-форма) при том, что реальный тул требует `cmd: string`, а `apply_patch_tool_type: None`. Промпт и схема расходятся → модель генерирует невалидные вызовы.
- **НАШ АНАЛОГ**: `crates/pa-core/src/tool_profile/prompt_templates.rs` + `facades.rs`.
- **ЧТО ДЕЛАТЬ**: **генерация** примеров из той же схемы, что отдаётся модели (single source of truth), плюс тест: вход — профиль инструментов; ожидание — каждый пример вызова в промпте валидируется против JSON-схемы соответствующего тула.
- **СЕРЬЁЗНОСТЬ**: high

### ID: streaming-parser-fails-open
- **ИСТОЧНИК**: issue [#45240](https://github.com/openai/codex/issues/45240); `codex-rs/apply-patch/src/streaming_parser.rs:154-173`
- **СИМПТОМ / ПРИЁМ**: **баг.** `StreamingPatchParser::finish()` особо обрабатывает последнюю строку `*** End Patch` (без `\n`) и ставит `mode = EndedPatch` **минуя** state machine. Следствия: (A) патч **без** `*** Begin Patch`, но с `*** End Patch` без перевода строки → `Ok(vec![])` вместо ошибки; (B) контент **после** `*** End Patch` молча проглатывается. Разница — только в завершающем `\n`. Это валидационный гейт, который «fail-open».
- **НАШ АНАЛОГ**: `pa-edit/src/modes/apply_patch.rs:209` `parse_apply_patch_streaming` + `session.rs` (стриминговый парсер).
- **ЧТО ДЕЛАТЬ**: тест на **оба** варианта с `\n` и без: вход — `"*** End Patch"` (без Begin) → ожидание `Err`; вход — `"…*** End Patch\n*** End Patch"` (без финального `\n`) → ожидание `Err`. И симметрия: результат не должен зависеть от наличия финального перевода строки.
- **СЕРЬЁЗНОСТЬ**: high

### ID: apply-patch-trailing-space-filename
- **ИСТОЧНИК**: issue [#53245](https://github.com/openai/codex/issues/53245)
- **СИМПТОМ / ПРИЁМ**: `apply_patch` **срезает** пробелы в конце имени файла → правка уходит не в тот файл (или в новый). `filename: /(.+)/` в грамматике, дальше `.trim()`.
- **НАШ АНАЛОГ**: `pa-edit/src/modes/apply_patch.rs:43` `strip_apply_patch_path_noise`, `text::js_trim`.
- **ЧТО ДЕЛАТЬ**: тест: вход — `*** Update File: "a.txt "` (пробел в конце); ожидание — либо явная ошибка `path has trailing whitespace`, либо правка `a.txt `; **никогда** тихая правка `a.txt`.
- **СЕРЬЁЗНОСТЬ**: medium

### ID: apply-patch-undefined-file
- **ИСТОЧНИК**: issue [#53906](https://github.com/openai/codex/issues/53906)
- **СИМПТОМ / ПРИЁМ**: **баг.** Патч, где один путь абсолютный (вне cwd), а другой — относительный, создаёт **пустой файл с именем `undefined`**. Т.е. при ошибке резолва пути создаётся мусорный файл вместо ошибки.
- **НАШ АНАЛОГ**: `pa-edit/src/path_policy.rs` (`PathPolicy`), `files.rs`.
- **ЧТО ДЕЛАТЬ**: тест: вход — патч со смесью absolute + relative путей; ожидание — либо явная ошибка политики путей, либо корректные файлы; **в дереве не появляется** файл `undefined` (и вообще ни одного файла с именем из «пустоты»).
- **СЕРЬЁЗНОСТЬ**: high

### ID: apply-patch-crlf-preserved
- **ИСТОЧНИК**: эксперимент `/tmp/cx/edge` (CRLF)
- **СИМПТОМ / ПРИЁМ**: **правильное поведение.** Файл `alpha\r\nmiddle\r\nomega\r\n`, патч с LF-строками → применён, **CRLF сохранён** (`b'ALPHA\r\nmiddle\r\nomega\r\n'`). В коде это `text_file.rs::SourceFile` (хранит терминатор на строке) + комментарий в `file_update.rs:100-107` про mixed line endings.
- **НАШ АНАЛОГ**: `pa-edit/src/text.rs`.
- **ЧТО ДЕЛАТЬ**: регрессионный тест: вход — CRLF-файл + LF-патч; ожидание — все переводы строк остались `\r\n`. Плюс mixed-EOL файл.
- **СЕРЬЁЗНОСТЬ**: medium

### ID: apply-patch-move-and-delete
- **ИСТОЧНИК**: эксперимент `/tmp/cx/mv`; `apply_patch.lark` (`change_move`)
- **СИМПТОМ / ПРИЁМ**: `*** Move to:` — это **часть `Update File`**, а не отдельная операция: `*** Update File: m.txt` + `*** Move to: m2.txt` + хунки. `Delete File` — отдельный хунк. `Add File` создаёт отсутствующие родительские директории (`deep/nested/n.txt` → ok, есть retry-хелпер `write_file_with_missing_parent_retry`).
- **НАШ АНАЛОГ**: `apply_patch.md` описывает `*** Move to:` так же; `FileOp`/`FileOpIntent` в `engine.rs`.
- **ЧТО ДЕЛАТЬ**: тесты на (1) move+update одной операцией, (2) move в несуществующую директорию, (3) `Add File` в отсутствующие родительские папки. Ожидание — во всех трёх успех и корректное содержимое.
- **СЕРЬЁЗНОСТЬ**: low

### ID: tool-search-bm25-deferred
- **ИСТОЧНИК**: `codex-rs/core/src/tools/handlers/tool_search_spec.rs:88-91`
- **СИМПТОМ / ПРИЁМ**: `tool_search` — отдельный тул с описанием «Searches over deferred tool metadata with BM25 and exposes matching tools for the next model call». Явно сказано: «For MCP tool discovery, **always** use `tool_search` instead of `list_mcp_resources`/`list_mcp_resource_templates`» — т.е. они **убирают** конкурирующий способ, а не надеются на послушность модели.
- **НАШ АНАЛОГ**: `crates/pa-core/src/tool_profile/disclosure.rs`.
- **ЧТО ДЕЛАТЬ**: перенять «убрать конкурента»: если у нас есть два способа найти символ (`graph.search` и `lsp.find_symbol`), в промпте должен быть **один** рекомендованный + явный запрет на второй для этого класса задач (как у нас уже сделано в `AGENTS.md` codebase-memory: «ALWAYS prefer MCP graph tools over grep/glob»).
- **СЕРЬЁЗНОСТЬ**: medium

### ID: prompt-permission-hierarchy
- **ИСТОЧНИК**: `/tmp/cx/instr_gpt6astra.md:169-2304` (из `codex debug models`)
- **СИМПТОМ / ПРИЁМ**: в промпте явно задан **порядок приоритета**: «The user's instruction … must take precedence over any guidelines provided in skills or external files». И: «You MUST complete the work that is already authorized … before asking the user for permission as a final step» + «The user gets very frustrated when you stop and ask for confirmation». Это борьба с «модель спрашивает разрешение вместо работы».
- **НАШ АНАЛОГ**: наши `AGENTS.md` / skill-инструкции; harness-state prompt-notes.
- **ЧТО ДЕЛАТЬ**: перенять формулировку приоритета в наш системный промпт (skill < user < system) и правило «сначала сделай ревьюабельный результат, потом спрашивай». Тест: вход — задача, где skill запрещает, а user разрешает; ожидание — агент следует user.
- **СЕРЬЁЗНОСТЬ**: medium

---

## 2. Что НЕ стоит перенимать (антипаттерны Codex)

| Приём Codex | Почему плохо |
|---|---|
| `apply_patch` без структурированных ошибок | модель ретраит тот же патч (см. `apply-patch-error-no-hint`, 83 dup-фейла/30 дней) |
| молчаливый выбор первого матча при неоднозначности | тихая правка не того места (`apply-patch-silent-ambiguity`) |
| `trim()`-фаззи в финальном проходе | тихая порча отступов (`apply-patch-fuzzy-overreach`) |
| отсутствие детектора tool-loop | 826 255 повторов в репро (`tool-loop-unbounded`) |
| `broadcast` с `continue` на `Lagged` | тихая потеря вывода (`exec-truncation-silent-drop-bug`) |
| отсутствие проверки висящих `session_id` на конце тёрна | потеря результата фонового процесса (`exec-background-turn-end`) |
| поле в структуре, но не в схеме (`timeout_ms`) | модель не знает о возможности (`exec-timeout-not-in-schema`) |
| `apply_patch` как shell-команда без бинаря на PATH | `command not found`, лечится tree-sitter-эвристикой (`exec-heredoc-apply-patch-detection`) |
| `Add File` на существующий файл = перезапись | потеря данных (у нас уже правильнее) |

---

## 3. 10 самых важных пунктов

1. **`tool-loop-unbounded` (critical).** Codex вообще не ограничивает повторяющиеся вызовы — подтверждено и репро на 826 255 повторов, и моим локальным логом (152 идентичных `write_stdin` подряд). Нам нужен детектор `hash(name+args)` в окне, предупреждение на 3-м повторе, обрыв на 6-м.
2. **`tool-loop-parse-error-no-abort` (critical).** Один битый вызов в батче не должен приводить к повторному исполнению валидных соседей (в апстриме — 1 126 побочных эффектов вместо 2).
3. **`parallel-opt-in-per-tool` + `parallel-same-file-conflict` (critical).** Параллельность — opt-in **на действие**, `RwLock` вокруг `apply_patch`. У нас `lsp` смешивает read (`references`) и write (`rename`) — нужен per-action флаг и блокировка по пути файла.
4. **`tool-output-context-blowup` (critical).** Один результат на 948 k токенов, 7.24 M input за 16 минут, 49 % пятичасового лимита. Нужен жёсткий потолок на tool-result в контексте + подсказка «сузь запрос».
5. **`call-output-pairing` (critical).** Валидация `call_id` **на границе отправки**, иначе один висящий вызов заклинивает тред навсегда.
6. **`apply-patch-fuzzy-overreach` (critical).** Тихая замена с потерей отступа в Python/YAML — это порча кода без ошибки. Наш `sloppy` должен быть явным выбором, а не дефолтом `apply_patch`.
7. **`apply-patch-atomic-vs-cli` + `apply-patch-duplicate-path` (critical/high).** У нас атомарность **уже есть** (`ATOMICITY_NOTICE`) и `Add`-on-existing **уже отвергается** — это надо закрепить тестами, чтобы не потерять при рефакторинге. Codex в CLI-пути не атомарен.
8. **`errors-are-plain-text` + `apply-patch-error-no-hint` (high).** Перейти к структурированным ошибкам `{code, path, hunk, hint, retryable}` + человекочитаемая строка. Сейчас и у нас, и у них модель получает «Failed to find expected lines» без единой подсказки.
9. **`tool-desc-structure-codex` + `tool-desc-deferred-discovery` (high).** Наши описания — плоские строки; у Codex — структура «что / когда / ограничения / пример» и 6 уровней экспозиции с BM25-поиском. Для `lsp` с 22 действиями это прямое улучшение.
10. **`exec-background-turn-end` + `exec-empty-poll-storm` (critical/high).** Фоновый процесс теряется при завершении тёрна, а пустые поллы копятся сотнями. Нужны: проверка висящих `session_id` на конце тёрна и сообщение `no new output after N polls` + бэкофф.

---

## Приложение: артефакты экспериментов

- `/tmp/cx/mock3.py` — mock-провайдер, отдающий заданные tool-calls по SSE (переиспользуемо для наших регрессионных тестов).
- `/tmp/cx/reqs2/req_1.json` — точный payload к модели: `additional_tools` + промпты + окружение.
- `/tmp/cx/instr_gpt6astra.md` — полный `instructions_template` (21 420 симв.).
- `/tmp/cx/sc_*/req_*.json` — 12 сценариев: `errs`, `patch`, `patch2`, `trunc`, `trunc2`, `misc`, `bg`, `cap`, `loop`, `parpatch`, `deny`, `deny2`, `schema`, `timeout`, `atomic`, `dup`, `wrongtool`, `stale`, `afterexit`, `shell`.
- `/root/.codex/sessions/2026/06/29/rollout-…12-53-46….jsonl` — реальный лог с 152 идентичными `write_stdin`.
