# Аудит: oh-my-pi (TypeScript) → prime-agent (Rust)

**Источник:** локальная копия `/tmp/omp-src` (read-only, коммит `3f020bd`) + GitHub `can1357/oh-my-pi`
(265 issues по запросу `lsp`, 100 PR). Прочитаны `packages/coding-agent/src/lsp/**`
(`client.ts` 1942 стр., `tool.ts` 1576, `edits.ts`, `diagnostics.ts`, `writethrough.ts`,
`mux/server.ts` 721, `mux/daemon.ts`, `mux/protocol.ts`, `lspmux.ts`), промпт `prompts/tools/lsp.md`,
тесты `test/tools/lsp-regressions.test.ts` (5906 стр., ~180 кейсов), `test/lsp-mux.test.ts`,
`test/tools/lsp-diagnostics-freshness.test.ts`, `test/tools/lsp-batching.test.ts`.

**Наша сторона (для аналогий):** `/root/prime-agent/crates/pa-lsp/src/*` (8640 стр.),
`crates/pa-graph/src/*`, `crates/pa-edit/src/*`, `crates/pa-core/src/tools/edit_hybrid.rs`.

Формат находки: ID · ИСТОЧНИК · СИМПТОМ · ПРИЧИНА · НАШ АНАЛОГ · РЕГРЕССИОННЫЙ ТЕСТ · СЕРЬЁЗНОСТЬ.

---

## 0. Как читать «НАШ АНАЛОГ»

У нас 4 тула (`lsp`, `graph`, `edit`, `jev`), у них — один `lsp` + `edit`/`write`/`ast_edit`.
Маппинг, который используется ниже:

| Их действие | Наш тул |
|---|---|
| `lsp rename` | `lsp.rename` |
| `lsp rename_file` | `lsp.rename_file` |
| `lsp references/definition/implementation` | `lsp.references` / `.definition` / `.implementation` / `.find_referencing_symbols` |
| `lsp code_actions` + `apply:true` | `lsp.code_actions` |
| `lsp diagnostics` | `lsp.diagnostics` |
| `lsp reload` | `lsp.reload` |
| `lsp request` (raw JSON-RPC) | `lsp.capabilities`/`lsp.reload` (raw-метода у нас нет) |
| их mux (`lsp.shared`) | `pa-lsp::mux` (LspMux/MuxIdentity) |
| `write`/`edit` writethrough | `pa-edit` session → `pa-lsp::writethrough` |
| (нет аналога, только у нас) | `graph.*` (`index`/`reindex`/`trace`/`architecture`/`coverage`/`changes`) |

---

## 1. Rename / rename_file / safe_delete

### F01 · `rename-consumer-stale-overlay`
- **ИСТОЧНИК:** https://github.com/can1357/oh-my-pi/issues/15772 (OPEN, 18.8.9; код на `2ee514247f`
  `lsp/tool.ts:1165-1168`, `1494-1519`, `lsp/edits.ts:163-166`)
- **СИМПТОМ:** `rename` вернул `success: true` и «Applied 2 edit(s) to b.ts», но в `b.ts` два
  идентификатора записались **в пустые строки** в начале файла; реальный импорт и оба вызова не
  изменились, декларация переименована → оба файла сломаны. `references` тоже показали 3 ссылки со
  старыми номерами строк вместо 4 с новыми.
- **ПРИЧИНА:** перед cross-file операцией синхронизируется только *целевой* файл
  (`reconcileFileFromDisk(targetFile)`), а уже открытый *потребитель* (`b.ts`) изменённый извне —
  нет. Сервер считает `WorkspaceEdit` по своей (старой) копии `b.ts`; клиент применяет диапазоны к
  текущему тексту на диске без сверки снапшота.
- **НАШ АНАЛОГ:** `lsp.rename` → `pa_lsp::client::apply_workspace_edit` (`client.rs:1193`) +
  `apply_text_edits` (`client.rs:1241`). У нас та же дыра в ослабленном виде: `ensure_open`
  перечитывает диск на каждый вызов, но `apply_text_edits` **никогда** не проверяет, что текст, к
  которому применяется диапазон, тот же, по которому сервер считал правку. `WorkspaceEdit` от
  сервера применяется «слепо» по номерам строк.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `rename_into_reformatted_consumer` — файл `a.ts` (`export function answer()`),
  `b.ts` (импорт + вызов). Открыть `b.ts` через `lsp.symbols` (сервер держит оверлей). Вне OMP
  переписать `b.ts`, добавив 2 пустые строки в начало и ещё один вызов. Вызвать
  `lsp.rename{a.ts, symbol:"answer", new_name:"respond", apply:true}`.
  **Ожидаемо:** либо все 4 вхождения переименованы по актуальным позициям, либо ошибка «документ
  разошёлся со снапшотом сервера, повторите» и **ни один файл не изменён**. Не ожидаемо: идентификатор
  в пустой строке / частичный ренам.
- **СЕРЬЁЗНОСТЬ:** critical (тихая порча кода + ложный `success`).

### F02 · `workspace-edit-no-version-preflight`
- **ИСТОЧНИК:** https://github.com/can1357/oh-my-pi/issues/8374 (OPEN; `types.ts:114-120`,
  `edits.ts:196-205,243-285`). Их фикс-ветка: PR #8391 (`fix(lsp): correct workspace edit failure
  handling`).
- **СИМПТОМ:** файл содержит `newer`; применяется `TextDocumentEdit` с `textDocument.version: 1`,
  который заменяет первые 3 символа на `OLD` → на диске `OLDer`. Версия проигнорирована.
- **ПРИЧИНА:** `version` объявлен в типе, но не входит в `WorkspaceEditOp` и не проверяется в
  preflight. LSP даёт это поле именно для отказа от устаревших правок.
- **НАШ АНАЛОГ:** `lsp.rename`, `lsp.code_actions(apply)`, `lsp.rename_file` → `apply_workspace_edit`
  (`client.rs:1193`). Мы версию **не читаем вообще** (`text_document.version` в
  `DocumentChanges::Edits` игнорируется), и preflight отсутствует: цикл `for (uri, edits) in changes`
  пишет файл сразу, до проверки остальных.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `workspace_edit_rejects_stale_document_version` — файл `a.txt` = `newer`;
  применить `WorkspaceEdit{document_changes:[{textDocument:{uri, version:1}, edits:[range 0..3 → "OLD"]}]}`,
  а `open_docs` для `a.txt` = 2 (или на диске уже другое содержимое).
  **Ожидаемо:** ошибка «stale document version» и файл = `newer`. Дополнительно: `version` отсутствует
  (None) → применяется (спека разрешает).
- **СЕРЬЁЗНОСТЬ:** high.

### F03 · `workspace-edit-not-atomic-and-late-bail`
- **ИСТОЧНИК:** https://github.com/can1357/oh-my-pi/issues/15525 (OPEN, 18.8.9: «EACCES на beta;
  alpha уже импортировал renamed_value, а core всё ещё определял shared_value. Ошибка не сообщила о
  мутации alpha»). Их же #8379 (`rename_file` оставляет применённые правки после неудачного move) и
  PR #12285 `fix(lsp): roll back failed reference writes`.
- **СИМПТОМ:** многофайловая правка падает на 3-м файле; первые два уже записаны, ошибка об этом не
  сообщает; откат отсутствует.
- **ПРИЧИНА:** `applyWorkspaceEdit` валидирует диапазоны всех файлов заранее, но **пишет по одному**,
  и на ошибке возвращает исключение без списка уже изменённых путей.
- **НАШ АНАЛОГ:** `apply_workspace_edit` (`client.rs:1193-1216`) — ещё хуже: ветка
  `DocumentChanges::Operations(_) => anyhow::bail!("file-operation workspace edits are not supported yet")`
  стоит **после** того, как `changes` и `DocumentChanges::Edits` уже записаны на диск. То есть
  «отказ» приходит после частичной мутации. Плюс `apply_text_edits` (`client.rs:1241`) пишет каждый
  файл отдельным `std::fs::write`.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `workspace_edit_refuses_before_any_write` — `WorkspaceEdit` с
  `changes: {a.txt: [...]}` **и** `document_changes: [Operations([CreateFile...])]`.
  **Ожидаемо:** ошибка и `a.txt` байт-в-байт как был. (Сейчас `a.txt` уже перезаписан.)
  Второй кейс: `multi_file_edit_reports_executed_prefix` — два файла, второй `chmod 444` → ошибка
  должна перечислить `a.txt` как **изменённый** (или откатить его).
- **СЕРЬЁЗНОСТЬ:** critical (заявленный отказ + фактическая мутация).

### F04 · `rename-file-swallows-willrename-failure`
- **ИСТОЧНИК:** https://github.com/can1357/oh-my-pi/issues/8380 (CLOSED, PR #8396) +
  https://github.com/can1357/oh-my-pi/issues/15526 (OPEN: «`rename_file` молча делает обычный move,
  когда семантическая помощь недоступна»; `tool.ts:603-607`, `610-631`, `641-646`).
- **СИМПТОМ:** `Renamed pkg/core.py → pkg/core2.py`, без предупреждения; три импорта по-прежнему
  ссылаются на `pkg.core`, которого больше нет. Preview в этом же сценарии печатает «No LSP edits
  would be applied», а apply — молчит.
- **ПРИЧИНА:** недоступный сервер превращается в «note» и не блокирует; `-32601` глушится молча;
  «успешный нулевой ответ» и «метод не поддержан» не различаются.
- **НАШ АНАЛОГ:** `lsp.rename_file` → `LspManager::rename_file` (`manager.rs:297-322`):
  ```rust
  if let Ok(Some(edit)) = client.rename_files(&renames).await { ... }
  ```
  Любая ошибка (включая `-32601`, таймаут, падение сервера, **и** ошибку применения правки внутри
  `apply_workspace_edit`) **молча проглатывается**, затем выполняется `fs::rename` и возвращается
  `Ok(touched)`. Инструмент рапортует «renamed X -> Y», даже если ни один импорт не обновлён.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `rename_file_aborts_when_willrename_fails` — фейковый сервер отвечает
  `-32000 "boom"` на `workspace/willRenameFiles` (или `-32601`). Вызвать
  `lsp.rename_file{file:"a.py", dest:"b.py"}`.
  **Ожидаемо:** `is_error: true`, текст содержит имя сервера и причину, `a.py` на месте, `b.py` нет.
  Отдельный кейс: сервер поддерживает метод и вернул `null` → move разрешён, но в выводе
  «0 reference edits» (а не тишина).
- **СЕРЬЁЗНОСТЬ:** critical.

### F05 · `rename-file-clobbers-destination`
- **ИСТОЧНИК:** https://github.com/can1357/oh-my-pi/issues/8381 (CLOSED, PR #9628: «treats every stat
  error as a missing path») + их `tool.ts` `enumerateRenamePairs`/`MAX_RENAME_PAIRS` и явные проверки
  source/dest (`stat` vs `lstat`, EACCES ≠ ENOENT, dangling symlink).
- **СИМПТОМ:** при ошибке `stat` (EACCES/ELOOP/EIO) инструмент считал путь отсутствующим и продолжал;
  при существующем destination — перезаписывал; на каталоге (>1000 файлов) — отказывался.
- **ПРИЧИНА:** не различались коды ошибок; `stat` вместо `lstat`; нет проверки существования
  назначения перед move.
- **НАШ АНАЛОГ:** `lsp.rename_file` (`tool.rs:236-252` + `manager.rs:313`). У нас:
  1. **нет проверки, что `to` не существует** — `tokio::fs::rename` на POSIX **молча перезапишет**
     существующий файл (потеря данных);
  2. **нет проверки, что `from` существует** — сообщение будет `rename failed: ENOENT` без указания,
     что именно отсутствует;
  3. **нет обхода каталога** — rename каталога с вложенными файлами отправит серверу один
     `FileRename` пары каталогов; `didRenameFiles` для детей не пошлётся.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `rename_file_never_overwrites_existing_destination` — `a.txt`="A",
  `b.txt`="B"; `lsp.rename_file{file:"a.txt", dest:"b.txt"}` → **ожидаемо** ошибка
  «destination exists», `b.txt` == "B", `a.txt` == "A". (Сейчас `b.txt` становится "A".)
  Плюс `rename_file_missing_source_names_the_missing_path` и
  `rename_file_directory_renames_each_child`.
- **СЕРЬЁЗНОСТЬ:** critical (необратимая потеря файла).

### F06 · `safe-delete-fails-open`
- **ИСТОЧНИК:** https://github.com/can1357/oh-my-pi/issues/8377 (CLOSED, PR #8397: «diagnostics
  reports OK when every applicable server fails») и #8387 (workspace symbol: «no matches» когда все
  серверы упали). Класс ошибки: «отказ инструмента схлопывается в пустой успешный результат».
- **СИМПТОМ:** при недоступном сервере инструмент возвращал «нет ссылок / нет совпадений» с
  `success: true` вместо ошибки.
- **ПРИЧИНА:** `catch {} → []`; пустой ответ сервера и полный отказ неразличимы.
- **НАШ АНАЛОГ:** `lsp.safe_delete_symbol` (`tool.rs:698-709`):
  ```rust
  let references = match self.manager.ensure_client(&path).await {
      Ok(client) => client.references(&path, position, false).await
                        .unwrap_or_default().unwrap_or_default(),
      Err(_) => Vec::new(),
  };
  if !references.is_empty() { refuse }
  ... delete
  ```
  Любой сбой LSP (нет сервера, таймаут, краш, `null`) → **пустой список ссылок → символ удаляется**.
  Это fail-open на необратимой операции.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `safe_delete_refuses_when_references_unknown` — фейковый сервер отвечает
  ошибкой (или не отвечает) на `textDocument/references`; вызвать
  `lsp.safe_delete_symbol{symbol:"foo", apply:true}`.
  **Ожидаемо:** `is_error: true`, текст «cannot verify references: <причина>», файл не изменён.
  Отдельно: сервер вернул `[]` → удаление разрешено (это законный нулевой ответ).
- **СЕРЬЁЗНОСТЬ:** critical.

### F07 · `safe-delete-single-server-only`
- **ИСТОЧНИК:** https://github.com/can1357/oh-my-pi/issues/8311 (OPEN: «references silently miss
  unopened TypeScript workspace packages»: cold 12 ссылок, после открытия consumer-файла — 17) и
  #15526 (семантика «поддерживающий сервер обязан ответить»).
- **СИМПТОМ:** `references` выглядит полным и рапортует success, но пропускает целые пакеты
  монорепы, пока в них не откроют файл.
- **ПРИЧИНА:** сервер отвечает только по загруженным проектам; клиент не различает «0 ссылок» и
  «0 ссылок в загруженной части».
- **НАШ АНАЛОГ:** `safe_delete_symbol` и `find_referencing_symbols` спрашивают **ровно один**
  клиент (`manager.ensure_client(&path)` — по расширению файла). Символ, на который ссылаются из
  другого языка/другого root (например, `.py`-функция, вызываемая из `.pyi`-стабов или из
  `.ts`-обёртки), не увидит ссылок вообще. `lsp.references` — то же.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `safe_delete_checks_every_owning_root` — символ определён в
  `pkg/core.py`, используется в `pkg2/consumer.py`, при этом `pkg` и `pkg2` — разные корни с разными
  `pyproject.toml` (два клиента). **Ожидаемо:** отказ с перечислением ссылок из обоих корней.
  Дополнительно `references_reports_partial_coverage`: если сервер не поддерживает
  `workspace/symbol`/не загрузил проект — в выводе явная пометка «partial», а не «Found N».
- **СЕРЬЁЗНОСТЬ:** high.

---

## 2. references / definition / implementation

### F08 · `request-timeout-is-decorative`
- **ИСТОЧНИК:** https://github.com/can1357/oh-my-pi/issues/5804 (CLOSED: таймаут был жёстко зажат
  60 с), #3962 (CLOSED, PR #3967: «cold-start initialize ignores the tool signal»), #8116 (OPEN:
  «timed-out LSP requests continue running on the server»), #3001 (CLOSED: серверы вешаются на
  старте), #4910 (OPEN: edit tool висит навсегда из-за LSP-пайплайна, `DIAGNOSTICS_PIPELINE_GRACE_MS`).
- **СИМПТОМ:** сервер зависает → запрос висит до внешнего убийства процесса; при таймауте
  `$/cancelRequest` серверу не отправляется, работа продолжается.
- **ПРИЧИНА:** дефолтный таймаут ставился в одном месте и терялся в других; отмена не
  централизована.
- **НАШ АНАЛОГ:** `lsp` **целиком**. `LspParams::timeout_secs()` (`types.rs:300`) документирован в
  схеме как «default 20, range 5-300», но `dispatch` прокидывает `Duration` только в
  `diagnostics`/`workspace_diagnostics`/`find_symbol`; в `goto`, `references`, `hover`, `rename`,
  `code_actions`, `safe_delete`, `capabilities`, `reload`, `rename_file` параметр называется
  `_timeout` (`tool.rs:472,507,540,591,605,669,762`) и **не используется**. Ни один
  `server.request(...)` в `client.rs` не обёрнут в `tokio::time::timeout`. `$/cancelRequest` в коде
  отсутствует полностью.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `every_action_honors_timeout` — фейковый сервер не отвечает на
  `textDocument/references`; `lsp.references{timeout:5}`.
  **Ожидаемо:** ошибка «timed out after 5s» через ~5 с (не через 20/бесконечность), и серверу ушёл
  `$/cancelRequest` с тем же id. Прогнать по всем 10 действиям с `_timeout`.
- **СЕРЬЁЗНОСТЬ:** critical (зависание агента; параметр, которому модель верит, не работает).

### F09 · `cold-start-no-index-wait`
- **ИСТОЧНИК:** https://github.com/can1357/oh-my-pi/issues/8707 (OPEN: «fresh session discovers
  TypeScript server but never invokes requested definition»), #8311 (OPEN), #5276 (CLOSED: Pyright
  висит, потому что initialize не отправляет `didChangeConfiguration`), #12889 (CLOSED:
  «diagnosticsOnEdit returns empty OK on first edit with a cold server»). Их решение:
  `waitForProjectLoaded` + `$/progress` + повторные попытки `REFERENCES_RETRY_COUNT = 2`,
  `REFERENCES_RETRY_DELAY_MS = 250` + `isOnlyQueriedDeclaration` (`diagnostics.ts:186-200`).
- **СИМПТОМ:** первый запрос на холодном сервере возвращает пустоту/только декларацию; тот же запрос
  после «прогрева» — полный результат. Выглядит как «нет ссылок», а не «ещё индексируется».
- **ПРИЧИНА:** клиент не ждёт `$/progress`/`window/workDoneProgress` и не повторяет запрос; пустой
  ответ принимается как окончательный.
- **НАШ АНАЛОГ:** `pa_lsp::client`. `NotificationRouter` (`client.rs:225-257`) реализует только
  `publish_diagnostics`, `show_message`, `log_message`; `$/progress` не обрабатывается вообще,
  `client_capabilities()` (`client.rs:1149`) не объявляет `window.workDoneProgress`, поля
  `project_loaded`/`wait_for_project_loaded` нет. `references` (`client.rs:509`) делает один запрос
  и возвращает что дали. `lsp.references` → `tool.rs:540-580`: одна попытка, без ретраев и без
  проверки «вернулась только декларация».
- **РЕГРЕССИОННЫЙ ТЕСТ:** `references_retries_until_project_loaded` — фейковый сервер: первый
  `textDocument/references` → `[declaration]`, затем шлёт `$/progress{kind:"end"}`, второй запрос →
  4 локации. `lsp.references`.
  **Ожидаемо:** «Found 4 reference(s)» (или явное «indexing, 1 of 4»), не «Found 1».
  Плюс `capabilities_declare_work_done_progress` — в `initialize` есть
  `window.workDoneProgress: true`, иначе серверы вообще не шлют прогресс.
- **СЕРЬЁЗНОСТЬ:** high.

### F10 · `position-column-zero`
- **ИСТОЧНИК:** их же фикс-логика `resolveSymbolColumn` (`utils.ts:650-690`) + #15857 (OPEN,
  «LSP symbol selection is token-aware…»: голый идентификатор требует whole-token совпадения,
  иначе `print` в `print_document` → `Symbol "print" not found`). Их `tool.ts` требует `symbol` для
  project-aware `references/rename/definition`, чтобы не сработал fallback на первый непробельный
  символ.
- **СИМПТОМ:** без `symbol` позиция берётся по первому непробельному символу строки — часто это
  декоратор/ключевое слово/параметр, и сервер возвращает «правдоподобно выглядящий, но чужой»
  результат.
- **ПРИЧИНА:** fallback на column = first-non-whitespace без проверки, что там вообще идентификатор.
- **НАШ АНАЛОГ:** `LspTool::target` (`tool.rs:164-171`):
  ```rust
  anyhow::ensure!(line >= 1, "`line` is 1-based");
  Ok((Position::new(line - 1, 0), None))
  ```
  Мы берём **column 0 — то есть буквально первый символ строки, включая отступ**. Для
  `    fn foo()` это пробел → rust-analyzer/gopls/pyright возвращают `null` или символ внешнего
  блока. Хуже: `lsp.definition{file, line:10}` на indented-строке молча даёт «(none)».
- **РЕГРЕССИОННЫЙ ТЕСТ:** `definition_on_indented_line_targets_the_identifier` — файл
  `fn outer() {\n    let value = 1;\n}`; `lsp.definition{file, line:2}` (без `symbol`).
  **Ожидаемо:** либо позиция первого непробельного символа (`let`), либо явная ошибка «pass
  `symbol`» — но **не** `(none)` и не позиция пробела. Второй кейс:
  `symbol_must_match_whole_token` — `symbol:"print"` на строке `print_document + print_document`
  → ошибка «not found», а не позиция `print_document`.
- **СЕРЬЁЗНОСТЬ:** high.

### F11 · `line-zero-underflow`
- **ИСТОЧНИК:** их `resolveSymbolColumn` (`utils.ts:650`: `Math.max(1, line)` — нормализация) +
  их же тесты `throws when occurrence is out of bounds` / `resolves the requested symbol occurrence`.
- **СИМПТОМ:** `line: 0` (или отрицательное, приведённое к u32) не диагностируется, а уезжает в
  арифметику.
- **ПРИЧИНА:** валидация `line >= 1` стоит только в ветке «без symbol».
- **НАШ АНАЛОГ:** `tool.rs:149-150` и `158-159`:
  ```rust
  Some(line) => pick_on_line(&matches, line - 1)?,
  ```
  `line: u32` из JSON. При `line: 0` в debug — panic «attempt to subtract with overflow», в release
  — `u32::MAX`, затем `pick_on_line` форматирует `line + 1` → снова overflow. `ensure!(line >= 1)`
  на строке 170 до этих веток **не доходит**, потому что `if let Some(symbol)` возвращает раньше.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `line_zero_is_rejected_not_panicking` —
  `lsp.definition{file, symbol:"foo", line:0}` и `lsp.find_symbol{symbol:"foo", line:0}`.
  **Ожидаемо:** `is_error: true`, текст «`line` is 1-based»; процесс не падает.
- **СЕРЬЁЗНОСТЬ:** high (panic в агенте).

### F12 · `ensure-open-masks-read-failure`
- **ИСТОЧНИК:** их `readTextFromDisk` (`utils.ts:22-26`, PR #14730: «refuse FIFOs and devices
  instead of hanging») + `ensureFileOpen` (`client.ts:900-960`: `catch (err) { if (isEnoent(err)) return; throw err; }`).
- **СИМПТОМ:** файл не читается (нет прав / это FIFO / удалён между stat и read) — клиент открывает
  документ с пустым текстом, сервер отвечает диагностиками/ссылками по пустому файлу.
- **ПРИЧИНА:** ошибка чтения не отличается от «файла нет»; FIFO-чтение вообще висит вечно.
- **НАШ АНАЛОГ:** `client.rs:377`:
  ```rust
  let text = tokio::fs::read_to_string(path).await.unwrap_or_default();
  ```
  Любая ошибка → `""` → `didOpen{text:""}`. Дальше: `lsp.definition` на нечитаемом файле вернёт
  «(none)», `lsp.diagnostics` — «0 diagnostics» (файл-то пустой), `safe_delete` — удалит символ,
  которого «нет». Плюс FIFO повесит `read_to_string` навсегда (нет `O_NONBLOCK`/stat-проверки).
- **РЕГРЕССИОННЫЙ ТЕСТ:** `unreadable_file_is_an_error_not_an_empty_document` — `chmod 000` (под
  non-root) или путь-каталог; `lsp.diagnostics{file}`.
  **Ожидаемо:** `is_error: true` с причиной; `didOpen` с `text:""` не отправлялся.
  Плюс `fifo_path_is_refused_quickly` — `mkfifo f.ts`, вызов должен упасть за <1 с.
- **СЕРЬЁЗНОСТЬ:** high.

---

## 3. Диагностики

### F13 · `publish-dropped-on-lock-contention`
- **ИСТОЧНИК:** их `client.ts:1290-1300` (осознанное решение: «best-effort, `try_lock`, a contended
  lock must never break the LSP stream») — и наш аналог, где это уже не «best effort», а потеря.
- **СИМПТОМ:** диагностика, пришедшая в момент, когда кто-то читал карту, исчезает навсегда (до
  следующей публикации сервера, которой может не быть для неизменённого документа).
- **ПРИЧИНА:** `try_lock` + `if let Ok(guard)` = drop при конкуренции.
- **НАШ АНАЛОГ:** `NotificationRouter::publish_diagnostics` (`client.rs:233-246`):
  ```rust
  if let Ok(mut guard) = state.try_lock() {
      guard.diagnostics.insert(...); guard.generation += 1; guard.version.insert(...);
  }
  ```
  У нас `tokio::sync::Mutex`, и `diagnostics()`/`published_diagnostics()`/`fresh_published_diagnostics()`
  держат его на время чтения. Гонка «публикация во время чтения» → публикация выброшена, `generation`
  не вырос, `wait_for_diagnostics` ждёт до таймаута и возвращает `None` → инструмент печатает
  «no diagnostics published within 20s» **на файле с ошибками**. Это же ломает
  `fresh_published_diagnostics` (остаётся старый снапшот → «clean»).
- **РЕГРЕССИОННЫЙ ТЕСТ:** `publish_during_read_is_not_lost` — захватить `notifications` мьютекс
  (или запустить `published_diagnostics` в цикле), отправить `publishDiagnostics` с ошибкой,
  отпустить, прочитать. **Ожидаемо:** диагностика на месте (сейчас — потеряна). Проверить, что
  `generation` вырос ровно на 1.
- **СЕРЬЁЗНОСТЬ:** critical (ложный «чисто» на реальных ошибках, недетерминированно).

### F14 · `pull-unchanged-and-partial-become-clean`
- **ИСТОЧНИК:** https://github.com/can1357/oh-my-pi/issues/8378 (OPEN: «missing diagnostic responses
  are collapsed into a clean empty result»), #15408 (OPEN: «false clean (OK) when a push-only server
  publishes nothing»), #5825/#10035/#9624 (pull-диагностики Roslyn), их фикс PR #15412
  (`prevented false clean diagnostics without a report`).
- **СИМПТОМ:** `lsp diagnostics` → `OK`, повторный вызов через секунду → 2 реальные ошибки
  (`tombi`/`pyright`). Плюс «pull вернул Unchanged» трактуется как «чисто».
- **ПРИЧИНА:** `requestDocumentDiagnostics` превращает ошибку в `undefined`; `waitForDiagnostics`
  превращает итоговый `undefined` в `[]`; три состояния (clean / timed_out / unavailable)
  схлопнуты в одно.
- **НАШ АНАЛОГ:** `client.rs:633-641`:
  ```rust
  DocumentDiagnosticReport::Unchanged(_) => Vec::new(),   // ← «ничего не изменилось» = «чисто»
  DocumentDiagnosticReportResult::Partial(_) => Ok(Some(Vec::new())), // ← то же
  Err(_) => Ok(None),                                     // ← молчаливый fallback на push
  ```
  `Unchanged` — это ответ «предыдущий результат всё ещё актуален», а `previous_result_id` мы шлём
  `None`, поэтому сервер вправе ответить `Unchanged` только если сам решит; но `Partial` — это
  **неполный** набор, который мы выдаём за пустой. И `Err(_) => None` означает: pull упал →
  молча берём push (которого может не быть) → `unwrap_or_default()` в `diagnostics()` (`client.rs:648-658`)
  → пустой вектор → инструмент печатает «0 diagnostics».
- **РЕГРЕССИОННЫЙ ТЕСТ:** `partial_pull_report_is_not_clean` — фейковый сервер отвечает
  `{kind:"partial", items:[]}` (или `Unchanged`) на `textDocument/diagnostic`.
  **Ожидаемо:** результат помечен как «incomplete/stale», `is_error:false`, но **не** «0 diagnostics /
  clean». Второй кейс: `pull_failure_is_surfaced` — сервер отвечает `-32603` на pull и ничего не
  публикует → «diagnostics unavailable», а не «clean».
- **СЕРЬЁЗНОСТЬ:** critical.

### F15 · `stale-version-publish-becomes-clean`
- **ИСТОЧНИК:** https://github.com/can1357/oh-my-pi/issues/8385/#12889 (свежесть публикации),
  #15408 (тот же симптом), их `waitForDiagnostics` (`diagnostics.ts:243-330`) с
  `minVersion`/`expectedDocumentVersion`/`settleMs` и явным комментарием «A failed pull leaves the
  file's state unknown; never let it collapse into a clean empty result».
- **СИМПТОМ:** публикация со старым `version` отбрасывается — но результат отбрасывания
  «пустой список», т.е. «чисто».
- **ПРИЧИНА:** фильтр свежести возвращает пустоту вместо «неизвестно».
- **НАШ АНАЛОГ:** `client.rs:944-954`:
  ```rust
  if !gate.accepts(published_version) {
      return Some((Vec::new(), generation));   // ← «устарело» → «чисто»
  }
  ```
  `FreshnessGate::accepts` (`diagnostics_flow.rs:175-180`) корректен, но вызывающая сторона
  превращает отказ в пустой успешный результат. Плюс `diagnostics()` (`client.rs:648`) вообще не
  использует гейт для pull-пути (`pull_diagnostics` возвращает что дали).
- **РЕГРЕССИОННЫЙ ТЕСТ:** `stale_publish_is_not_reported_as_clean` — `open_docs[a.ts]=5`, сервер
  публикует `publishDiagnostics{uri:a.ts, version:3, diagnostics:[]}`.
  **Ожидаемо:** состояние «stale/unknown» (и `wait_for_diagnostics` продолжает ждать до свежей
  публикации), а не `0 diagnostics`.
- **СЕРЬЁЗНОСТЬ:** high.

### F16 · `uri-key-mismatch`
- **ИСТОЧНИК:** https://github.com/can1357/oh-my-pi/issues/7662 (CLOSED, PR #7663: «published
  diagnostics dropped when a server renormalizes file URIs (Windows drive-letter/percent-encoding) —
  marksman always reports OK»). Их решение: `EquivalentUriMap` (`utils.ts:76-101`).
- **СИМПТОМ:** сервер нормализует URI (`file:///c%3A/...`, `file:///C:/...`,
  `file://localhost/...`, `%20` vs пробел) → диагностики не находятся → «OK».
- **ПРИЧИНА:** ключ карты — сырая строка URI.
- **НАШ АНАЛОГ:** `ClientNotifications.diagnostics: HashMap<String, Vec<Diagnostic>>` и
  `published_diagnostics` (`client.rs:844-852`) строят ключ как
  `Url::from_file_path(path)?.to_string()`; серверная сторона пишет `params.uri.to_string()`. Любое
  расхождение (percent-encoding, регистр диска, `localhost`, симлинк) → `None` → «clean».
  `pa_lsp` не имеет аналога `EquivalentUriMap`.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `diagnostics_match_reencoded_uri` — открыть
  `C:\proj\a b.ts` (или `/tmp/a b.ts`), сервер публикует для `file:///tmp/a%20b.ts` с ошибкой.
  **Ожидаемо:** ошибка найдена. Второй кейс: `file:///c%3A/...` vs `file:///C:/...` на win32.
- **СЕРЬЁЗНОСТЬ:** high.

### F17 · `diagnostics-stale-after-sibling-write`
- **ИСТОЧНИК:** https://github.com/can1357/oh-my-pi/issues/12793 (CLOSED), #12924 (CLOSED: «writethrough
  sends didOpen before the file exists on disk»), #4459 (CLOSED, PR #4462: «servers are never notified
  of harness-written files — no didChangeWatchedFiles»), #11416 (CLOSED, PR #11419: «external edits can
  leave semantic queries using a stale document»), #4459-класс в `writethrough.ts:1580-1595`.
- **СИМПТОМ:** `write` создаёт `foo.types.ts` → диагностика импортёра A навсегда показывает
  `Module has no exported member 'X'` / `Property 'section' does not exist on type '{}'`, хотя
  `tsc --noEmit` чист. Новый файл с тем же импортом резолвится сразу.
- **ПРИЧИНА:** (а) `didChangeWatchedFiles` не отправлялся вообще; (б) инвалидировался кэш
  диагностик только для *созданного* URI, не для импортёров; (в) `didOpen` уходил раньше, чем файл
  появлялся на диске.
- **НАШ АНАЛОГ:** `pa-lsp`. `did_change_watched_files` **объявлен** (`client.rs:1037`) но **не
  вызывается ниоткуда** (`grep` даёт только определение). `pa_graph::fs_watcher` умеет строить
  `DidChangeWatchedFiles` (`fs_watcher.rs:229`), но `pa-graph` не подключён к `pa-lsp`. В
  `writethrough.rs:216-243` шаги идут `SyncContent` (ensure_open, читает диск) → `Format` →
  `WriteDisk` → `NotifySaved`; для создания файла `Fswriter::write` сначала пишет файл, потом зовёт
  `write_through` — но `didSave` идёт без `didChangeWatchedFiles`, и ни один другой клиент
  (например, tsserver для соседнего пакета) не узнаёт о новом файле.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `created_module_clears_importer_diagnostics` — `a.ts` импортирует
  `./b`; `lsp.diagnostics{a.ts}` → 1 ошибка TS2307. Создать `b.ts` через `edit`/`write`.
  Повторить `lsp.diagnostics{a.ts}`.
  **Ожидаемо:** 0 ошибок (или явное «stale»), и в лог ушёл `workspace/didChangeWatchedFiles`
  с `type:1` для `b.ts`. Сейчас — та же ошибка навсегда.
- **СЕРЬЁЗНОСТЬ:** high.

### F18 · `diagnostics-of-wrong-file`
- **ИСТОЧНИК:** https://github.com/can1357/oh-my-pi/issues/8377/#8385 (workspace diagnostics
  проверяет только первый язык) — класс «ответ не по тому файлу/скоупу».
- **СИМПТОМ:** инструмент печатает диагностики одного файла как диагностики другого.
- **ПРИЧИНА:** переиспользование общего состояния без привязки к запрошенному URI.
- **НАШ АНАЛОГ:** `LspTool::diagnostics` (`tool.rs:258-300`): `client.ensure_client(&path)` выбирает
  клиент по расширению, но `flat_symbols(&path)` для атрибуции к символам может вернуть символы
  **другого** файла, если `ensure_open` отправил пустой текст (F12) — тогда все диагностики уедут в
  бакет «file-level». Более прямой баг: группировка `ledger::group_by_severity_and_symbol` использует
  `diag.range.start`, но не проверяет, что `diagnostic.range` внутри файла; диагностика с
  `range` за концом файла (сервер посчитал по другой версии) атрибутируется последнему символу.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `diagnostics_outside_file_range_are_file_level` — публикация с
  `range.start.line = 9999` для 10-строчного файла.
  **Ожидаемо:** бакет «file-level», без паники и без привязки к последнему символу.
- **СЕРЬЁЗНОСТЬ:** medium.

---

## 4. Правки (edit / patch / hashline / replace)

### F19 · `multi-file-edit-not-transactional`
- **ИСТОЧНИК:** https://github.com/can1357/oh-my-pi/issues/6366 (OPEN: «failed patch commits can leave
  files and snapshot heads partially mutated»; `Patcher.apply` пишет секции по одной, «Sections already
  written» без откката; порядок remove/move/write разделяет ФС и снапшоты), #15525 (OPEN), #15119
  (OPEN: правка, ломающая парсинг, применяется и остаётся).
- **СИМПТОМ:** многофайловый патч падает на 2-й секции — 1-я уже на диске; снапшот-голова обновлена
  для файла, который не записался; `MV` переехал в снапшоте, но не на диске.
- **ПРИЧИНА:** commit-фаза не транзакционная; каждый шаг (delete → invalidate, relocate → move)
  разрывает согласованность.
- **НАШ АНАЛОГ:** `edit` → `pa_edit::session::Session::apply` (`session.rs:291-...`). Док-комментарий
  прямо признаёт: *«A writer failure aborts the loop; files already written stay written»*
  (`session.rs:283-284`). Снапшоты пишутся **после** каждого write внутри того же цикла
  (`session.rs:337-367`), поэтому падение на 2-м файле оставляет 1-й файл изменённым **и** со свежим
  тегом, а 2-й — нет. Для hashline это значит: модель видит «частично применено», но без списка.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `multi_file_edit_is_all_or_nothing` — payload с двумя файлами
  (`a.txt`, `b.txt`), где `b.txt` недоступен для записи (каталог только для чтения / файл под
  `chmod 444`).
  **Ожидаемо:** `is_error: true`, `a.txt` байт-в-байт прежний (или, если откат невозможен, в тексте
  ошибки явный список `changed: a.txt` и старый тег `a.txt` невалиден).
  Второй кейс: `hashline_move_failure_restores_both_sides` — `MV a b` с падением на rename.
- **СЕРЬЁЗНОСТЬ:** critical.

### F20 · `hashline-applies-parse-regression`
- **ИСТОЧНИК:** https://github.com/can1357/oh-my-pi/issues/15119 (OPEN, 18.8.6: `PUT 7-8` с концом
  диапазона внутри узла применяется; файл перестаёт парситься; два warning'а, но `is_error:false`;
  следующий edit идёт по сдвинутым номерам).
- **СИМПТОМ:** диапазон разрезан посреди синтаксического узла → на диске битый файл, инструмент
  предупреждает, но не откатывает.
- **ПРИЧИНА:** `parses_cleanly` используется как источник warning'а, а не как вето; `autoRepair`
  по умолчанию выключен и в любом случае «угадывает после плохой записи».
- **НАШ АНАЛОГ:** `pa_edit::modes::hashline::apply::apply_edits` (`apply.rs:1187-1228`):
  ```rust
  let finish = |result, mut warnings| {
      if !parses_cleanly(options.path, &result.0) && baseline {
          warnings.push(edit_broke_parse_warning(result.1));   // ← только warning
      }
      ...
  };
  ...
  Ok(finish(authored, leading))     // ← применяется как есть
  ```
  То есть у нас ровно то же поведение: `is_error:false` + warning, файл остаётся сломанным.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `hashline_range_splitting_a_node_is_rejected` — TSX-файл из issue
  (`useMemo(...)` с многострочным массивом), `read` → `PUT 7-8:` с телом, закрывающим массив.
  **Ожидаемо:** `is_error: true`, файл не изменён, текст ошибки называет строку-конец и предлагает
  `PUT 7.=9`. (Сейчас: warning + битый файл.)
- **СЕРЬЁЗНОСТЬ:** critical.

### F21 · `fuzzy-fallback-silent-misalignment`
- **ИСТОЧНИК:** https://github.com/can1357/oh-my-pi/issues/10454 (OPEN, 18.0.11: «fuzzy anchor fallback
  silently applies a misaligned edit — no error, corrupted file»: заголовок функции выпал, тела
  продублировались, файл не парсится, инструмент сказал «applied»), #7432 (CLOSED: бесконечный цикл
  `replaceText` fuzzy-`all` при `newText ⊃ oldText`), #2603 (CLOSED: аудит hashline на indentation).
- **СИМПТОМ:** устаревший anchor не совпал → fuzzy-поиск нашёл «ближайший» блок → вставил текст не
  туда, без ошибки; порча обнаруживается только следующим edit'ом.
- **ПРИЧИНА:** `closest` при `confidence >= threshold` подставляется вместо `match`; в `all`-ветке
  цикл крутится до `adjusted === actual`, что не покрывает self-referential замену.
- **НАШ АНАЛОГ:** `edit` в режиме `sloppy` (`pa-core/src/tools/edit_hybrid.rs:168`:
  `allow_fuzzy: mode == EditMode::Sloppy`, порог `0.8`). В `sloppy/apply.rs:1208-1230`:
  ```rust
  if result.candidates.is_empty() && !result.overflow && !marker_op {
      result = collect_candidates(..., MatchMode::Fuzzy, false);   // ← молчаливый fuzzy
  }
  ```
  и `fuzzy_occurrences` (`apply.rs:402-440`) — эвристика: ищет 3 «сида» по 5 символов и пробует
  ±`limit` смещений. Она может попасть в **другую** функцию с похожим кодом. При `candidates.len() <= 4`
  и одинаковом результате для всех кандидатов (`apply.rs:1285-1296`) выбирается `candidates[0]`
  **без** пометки, что это fuzzy-догадка.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `sloppy_anchor_mismatch_refuses_instead_of_guessing` — файл с двумя почти
  идентичными функциями `handleExportCsv` и `handleExportExcel`; anchor содержит заголовок
  `const handleExportExcel = async () => {`, которого в файле уже нет (первая правка его убрала).
  **Ожидаемо:** `is_error: true` + «closest match at line N (87% similar)» + diff, файл не изменён.
  Приемлемо как компромисс: применить, но с явным маркером `[fuzzy match at line N]` в результате.
- **СЕРЬЁЗНОСТЬ:** critical.

### F22 · `fuzzy-normalization-fullwidth`
- **ИСТОЧНИК:** https://github.com/can1357/oh-my-pi/issues/16113 (OPEN, 18.11.1, движок `pi-edit` —
  **наш же**): `normalize_for_fuzzy` (`crates/pi-edit/src/text.rs`) не сворачивает
  Halfwidth/Fullwidth Forms (U+FF01–U+FF5E), similarity прыгает 12%–93%, а diff показывает две
  визуально идентичные строки.
- **СИМПТОМ:** правка CJK-файла с `（ ） ＝` не находит anchor; модель ретраит то же самое.
- **ПРИЧИНА:** фолдятся только кавычки/тире/пробелы; каждый fullwidth-символ даёт 1 к edit distance,
  поэтому score зависит от длины строки.
- **НАШ АНАЛОГ:** `pa-edit/src/text.rs:299-325` (`normalize_for_fuzzy`) — **тот же список**:
  `" ' ` ´` → кавычки, `‐‑‒–—−` → `-`, `' '`/`\t` → пробел. Fullwidth не трогается.
  `similarity()` (`fuzzy.rs:423`) — среднее по строкам Levenshtein по `chars`.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `fuzzy_matches_fullwidth_punctuation` — файл содержит `foo（bar）＝1`,
  anchor с halfwidth `foo(bar)=1`. **Ожидаемо:** матч найден (после NFKC-фолда на scoring-пути) и
  правка применена к правильной строке. Плюс `match_error_shows_codepoint_diff`: при отказе в тексте
  есть `differs at col 12: U+FF08 （ vs U+0028 (`.
- **СЕРЬЁЗНОСТЬ:** medium (но «застревание модели» дорого).

### F23 · `crlf-lf-asymmetry`
- **ИСТОЧНИК:** их `edit/normalize.ts` (`detectLineEnding`/`restoreLineEndings`) + тесты
  `splices CRLF files and preserves CRLF line endings` (`conflict-detect.test.ts:303`),
  `preserves a UTF-8 BOM when batch formatting changes content` (`lsp-batching.test.ts:115`),
  #7744 (CLOSED/OPEN: «Files are always created without the newline at end of file»).
- **СИМПТОМ:** после правки CRLF-файла он становится LF (или наоборот); BOM теряется; final newline
  пропадает.
- **ПРИЧИНА:** нормализация без восстановления; запись «как есть».
- **НАШ АНАЛОГ:** у нас это **сделано правильно** в `edit`: `FileRead::persist` (`files.rs:50-62`)
  восстанавливает BOM и `LineEnding`, есть тесты `file_hash_matches_typescript` с CRLF-кейсами.
  Но в **LSP-пути** — нет: `apply_text_edits` (`client.rs:1241`) читает `read_to_string` (сохраняет
  `\r`) и пишет `std::fs::write` — здесь ок; а `writethrough::apply_text_edits`
  (`writethrough.rs:163-190`) применяет formatting-edits через `LineIndex`/`byte_offset`, где
  `byte_offset` считает UTF-16 по строке, **включающей `\r`** — при `character`, равном длине строки
  без CR, вставка попадает перед `\r` (ок), но сервер, приславший `character = len+1`, вставит
  между `\r` и `\n` и разорвёт CRLF. Отдельно: `file_hash` (`store.rs:84-96`) **срезает `\r`**,
  поэтому CRLF- и LF-версии одного текста дают **один и тот же тег** — а тег используется для
  восстановления пути (`paths_with_hash`, `patcher.rs:106-123`) → возможна путаница двух файлов с
  одинаковым содержимым и разными переводами строк.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `formatting_edits_preserve_crlf` — CRLF-файл, сервер возвращает
  `formatting` edit с `range.end.character == line_len` (без учёта `\r`).
  **Ожидаемо:** файл остаётся CRLF, `\r` не отделён от `\n`.
  Плюс `identical_content_different_eol_has_distinct_tags` — `a.txt` (LF) и `b.txt` (CRLF) с
  одинаковым текстом: теги должны различаться (или recovery по тегу не должен переключать путь).
- **СЕРЬЁЗНОСТЬ:** medium.

### F24 · `hashline-noop-accepted`
- **ИСТОЧНИК:** их `NOOP_HARD_LIMIT = 3` + тест `shared_store_escalates_a_repeated_hashline_noop`;
  #6366 (частичные коммиты).
- **СИМПТОМ:** правка, не меняющая ни байта, рапортует успех; модель зацикливается.
- **ПРИЧИНА:** нет различения «применилось» и «ничего не изменилось».
- **НАШ АНАЛОГ:** `apply.rs:1228` `Ok(finish(authored, leading))` — если `authored` совпал с
  исходным текстом, вернётся `Ok` без ошибки. У нас есть тест на no-op для `replace`
  (`edit_hybrid.rs:565-585`) и на эскалацию **повторного** hashline-no-op (`:588-620`), но
  **одиночный** hashline-no-op не отличается от успеха.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `hashline_single_noop_is_flagged` — `PUT 1.=1:` с телом, идентичным
  строке 1. **Ожидаемо:** `is_error: true` или явный маркер «no-op» в результате (сейчас — успех).
- **СЕРЬЁЗНОСТЬ:** medium.

---

## 5. Граф / индекс

### F25 · `qn-collision-across-languages`
- **ИСТОЧНИК:** upstream-аналога нет (у oh-my-pi нет своего графа — только issue #4479 «Built-in
  graph-based code intelligence backend»). Класс взят из задачи «символы-дубликаты из разных языков»
  и подтверждён по нашему коду.
- **СИМПТОМ:** два разных файла-однофамильца дают один узел; символ «исчезает» из своего файла,
  рёбра уезжают в чужой файл, `graph.coverage`/`architecture` считают не то.
- **ПРИЧИНА:** `module_of(rel)` = путь без расширения с `::` (`indexer.rs:408-411`), поэтому
  `src/a.rs` и `src/a.py` дают префикс `src::a`; `qualified_name` = `src::a::foo` в обоих случаях;
  схема объявляет `UNIQUE (project, qualified_name)` (`schema.rs:38`), а `upsert_node`
  (`store.rs:171-193`) на конфликте **перезаписывает** `file_path`/`start_line`/`label` — то есть
  последний проиндексированный файл «крадёт» узел.
- **НАШ АНАЛОГ:** `graph.index`, `graph.reindex`, `graph.search`, `graph.trace`,
  `graph.architecture`, `graph.coverage`, `graph.changes` — всё, что читает `nodes`.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `same_symbol_name_in_two_languages_stays_two_nodes` — `src/a.rs` с
  `pub fn parse()`, `src/a.py` с `def parse():`; `graph.index`.
  **Ожидаемо:** два узла (`file_path` различаются), `graph.find` возвращает оба, `graph.coverage`
  считает 2 файла. (Сейчас — один узел, `file_path` = последний.)
- **СЕРЬЁЗНОСТЬ:** critical (тихая потеря символов + неверные рёбра).

### F26 · `incremental-reindex-skips-coverage`
- **ИСТОЧНИК:** upstream-аналога нет; класс — «инкрементальный reindex теряет/портит состояние»
  (их #6366 — частичное состояние после неудачного коммита; #15525 — «undisclosed partial writes»).
- **СИМПТОМ:** `graph.coverage` после `graph.reindex` показывает «partial/skipped» для файла,
  который уже парсится чисто; `complete: false` навсегда; `parsed` схлопывается в 0.
- **ПРИЧИНА:** `reindex_incremental` (`incremental.rs:132-306`) **не вызывает**
  `clear_file_coverage`/`record_coverage` (в отличие от `index_project`, `indexer.rs:130-150`), и не
  записывает skip-причины для oversized/unreadable/unsupported (там просто `continue`,
  `incremental.rs:159-171`). При этом `coverage()` (`coverage.rs:219-269`) берёт `total` из
  `file_hashes`, а `partial/unusable/skipped` — из `index_coverage`, и считает
  `parsed = total.saturating_sub(accounted)`: если заметок больше, чем хэшей (а это возможно —
  пропущенные файлы получают заметку, но не хэш), `parsed` насыщается в 0 → «0 из 3 распарсено».
- **НАШ АНАЛОГ:** `graph.reindex` + `graph.coverage` (и `graph.status`).
- **РЕГРЕССИОННЫЙ ТЕСТ:** `incremental_reindex_refreshes_coverage` — файл с синтаксической ошибкой →
  `graph.index` → `coverage.complete == false`, `partial == 1`. Исправить файл → `graph.reindex` →
  **ожидаемо** `complete == true`, `partial == 0`. (Сейчас заметка остаётся.)
  Второй кейс: `coverage_denominator_matches_notes` — проект из 3 файлов, один oversized →
  `total` должен включать его (или `skipped` не должен попадать в знаменатель) так, чтобы
  `parsed + partial + unusable + skipped == total`.
- **СЕРЬЁЗНОСТЬ:** high (инструмент врёт про собственную полноту).

### F27 · `suffix-call-resolution-false-edges`
- **ИСТОЧНИК:** upstream: #4479 (RFC на граф) — прямых баг-репортов нет; класс «неверные рёбра /
  ложные циклы» из задачи.
- **СИМПТОМ:** `graph.architecture{view:"cycles"}` показывает цикл между файлами, которых нет;
  `graph.trace` уходит в чужой модуль.
- **ПРИЧИНА:** `Resolver::resolve_call` (`indexer.rs:433-466`): при нескольких кандидатах с одним
  именем берётся `candidates[0]` (отсортированный по QN) со `strategy: "suffix"` — это **произвольный**
  выбор, а не «неоднозначно». Ребро пишется в граф как обычный `CALLS`; `architecture::fold_to_files`
  (`architecture.rs:288-300`) сворачивает `CALLS` в файловые рёбра без учёта confidence →
  `cycles()` (SCC по petgraph) выдаёт ложный цикл.
- **НАШ АНАЛОГ:** `graph.trace`, `graph.architecture` (`cycles`, `layers`, `hotspots`), `graph.search`.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `ambiguous_call_does_not_create_a_cycle` — `a.rs: fn parse()`, `b.rs:
  fn parse()`, `c.rs: fn run() { parse() }`; `graph.index` → `graph.architecture{view:"cycles"}`.
  **Ожидаемо:** либо пусто, либо ребро помечено `confidence` ниже порога и **не** участвует в SCC;
  `graph.trace{c.rs::run}` сообщает «ambiguous: 2 candidates», а не один произвольный.
- **СЕРЬЁЗНОСТЬ:** high (ложные циклы — прямой вред для решений агента).

### F28 · `edge-unique-collapse`
- **ИСТОЧНИК:** класс «неверные рёбра»; у нас — следствие схемы.
- **СИМПТОМ:** `fan_in`/`fan_out` в `graph.architecture{view:"hotspots"}` и `weight` в `structure`
  занижены; два вызова из `foo` в `bar` дают одно ребро.
- **ПРИЧИНА:** `UNIQUE (source, target, edge_type, local_name_gen)` (`schema.rs:53-56`) +
  `INSERT OR IGNORE` (`store.rs:215-219`). Для `CALLS` `props.local_name` отсутствует → `''` → все
  вызовы между той же парой узлов схлопываются в 1.
- **НАШ АНАЛОГ:** `graph.architecture` (hotspots/structure), `graph.coverage`, `graph.trace`
  (счётчики), `graph.changes`.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `repeated_calls_keep_their_weight` — `foo()` вызывает `bar()` дважды;
  `graph.architecture{view:"structure"}` → `weight == 2` (или явное поле `call_sites: 2`).
- **СЕРЬЁЗНОСТЬ:** medium.

### F29 · `trace-hides-recursion`
- **ИСТОЧНИК:** класс «ложные/потерянные рёбра».
- **СИМПТОМ:** `graph.trace` на рекурсивной/циклической функции не показывает обратное ребро —
  цикл невидим; нельзя отличить «нет пути» от «путь замкнулся».
- **ПРИЧИНА:** `trace` (`trace.rs:145-179`) использует `visited: HashSet<i64>` и `if !visited.insert(edge.target) { continue; }`
  — обратное ребро отбрасывается молча.
- **НАШ АНАЛОГ:** `graph.trace`.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `trace_reports_a_back_edge` — `a→b→a`; `graph.trace{a, depth:3}`.
  **Ожидаемо:** шаг с пометкой `cycle: true` (или отдельное поле `cycles`), а не тихое отсутствие.
- **СЕРЬЁЗНОСТЬ:** medium.

### F30 · `changes-loses-renamed-file-symbols`
- **ИСТОЧНИК:** upstream: `detect_changes`-аналога нет; класс «устаревший индекс / неверные рёбра».
- **СИМПТОМ:** `graph.changes` после `git mv` показывает файл, но **ни одного** затронутого символа —
  выглядит как «в файле ничего не поменялось».
- **ПРИЧИНА:** `detect_changes` (`changes.rs:238-254`) для `Renamed` читает hunks по **новому** пути,
  а `collect_symbols` (`changes.rs:287-300`) ищет `file_nodes[path]` — узла с новым путём в графе ещё
  нет (переиндексация не выполнялась) → `if !file_nodes.contains_key(path) { return Ok(()) }` —
  символы молча пропускаются.
- **НАШ АНАЛОГ:** `graph.changes`.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `changes_after_git_mv_are_not_empty` — граф построен, затем `git mv a.rs b.rs`
  + правка тела в `b.rs`, `graph.changes{from,to}` без reindex.
  **Ожидаемо:** символы перечислены (по старому пути/через `old_path`) **или** явное предупреждение
  «file not in index, run graph.reindex».
- **СЕРЬЁЗНОСТЬ:** medium.

---

## 6. Позиции: 0-based/1-based, UTF-16, концы файлов

### F31 · `offset-of-clamps-instead-of-erroring`
- **ИСТОЧНИК:** их `applyTextEditsToString` (`edits.ts:29-49`) использует `lines[start.line] || ""`,
  т.е. **тоже** клампит; но у них есть `sortAndValidateTextEdits` с явной проверкой перекрытий и
  `ToolError("overlapping LSP edits…")`. Их тесты `throws when occurrence is out of bounds`,
  `rejects unexpected snippet text edits`.
- **СИМПТОМ:** правка с диапазоном за концом файла «успешно» применяется в конец файла.
- **ПРИЧИНА:** конвертер позиции насыщает, а не сигналит.
- **НАШ АНАЛОГ:** `client.rs:1279-1299` `offset_of`: если `position.line >= число строк` →
  `return text.len()`; если `character` больше длины строки → `line_start + line_end`. Затем
  `anyhow::ensure!(start <= end && end <= text.len())` **проходит** (значения уже клампнуты).
  Итог: `TextEdit` на строку 9999 дописывается в конец файла — и это в `lsp.rename`,
  `lsp.code_actions(apply)`, `lsp.rename_file`.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `edit_beyond_eof_is_rejected` — 3-строчный файл, `TextEdit` с
  `range.start.line = 9999`. **Ожидаемо:** ошибка «range out of bounds: line 10000 > 3», файл не
  изменён. (Сейчас: текст дописан в конец.)
  Плюс `edit_mid_emoji_column_is_rejected_or_snapped` — строка `a😀b`, `character: 2` (середина
  суррогатной пары): ожидаем снап к границе символа и пометку, а не тихую вставку внутрь пары.
- **СЕРЬЁЗНОСТЬ:** high.

### F32 · `no-position-encoding-negotiation`
- **ИСТОЧНИК:** их `CLIENT_CAPABILITIES` (`client.ts:74-190`) тоже не объявляет
  `general.positionEncodings` — и это нигде не всплыло как issue, потому что они работают в UTF-16
  по умолчанию. Наш риск выше, потому что tree-sitter даёт байты.
- **СИМПТОМ:** сервер, предпочитающий `utf-8` offsets (или `utf-32`), и клиент, считающий UTF-16,
  дают сдвиг колонки на каждый не-ASCII символ слева от позиции.
- **ПРИЧИНА:** кодировка не согласована в `initialize`; предполагается UTF-16.
- **НАШ АНАЛОГ:** `pa-lsp`. `client_capabilities()` (`client.rs:1149-1182`) не содержит
  `general.positionEncodings`, и `ServerCapabilities.position_encoding` не читается.
  `position_at`/`byte_offset` (`text.rs:49-90`) жёстко считают UTF-16. Это **правильный дефолт**
  (спека: при отсутствии договорённости — UTF-16), но нет ни объявления, ни проверки: если сервер
  всё же ответит `positionEncoding: "utf-8"`, мы будем молча считать неверно.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `initialize_declares_utf16_position_encoding` — в `initialize` есть
  `capabilities.general.positionEncodings == ["utf-16"]`.
  Плюс `server_utf8_encoding_is_detected_or_refused` — сервер отвечает
  `capabilities.positionEncoding: "utf-8"` → клиент либо переключает конвертацию, либо падает с
  явной ошибкой «unsupported position encoding» (не молча).
- **СЕРЬЁЗНОСТЬ:** medium.

### F33 · `workspace-edit-uses-one-encoding-assumption-per-file`
- **ИСТОЧНИК:** их `applyTextEditsToString` оперирует строками JS (UTF-16 нативно) — у них этой
  проблемы нет by construction; у нас есть, потому что Rust-строки байтовые.
- **СИМПТОМ:** правка «на одну колонку левее» на строке с эмодзи/CJK перед позицией.
- **ПРИЧИНА:** две разные реализации конвертации в кодовой базе, с разной обработкой границ.
- **НАШ АНАЛОГ:** у нас **три** реализации: `text.rs::position_at/byte_offset` (через `LineIndex`),
  `client.rs::offset_of` (свой цикл по `char_indices`), `symbol_layer::range_from_bytes`. Первые две
  расходятся на граничных случаях (`offset_of` возвращает `line_start + line_end` при
  `character > len`, `byte_offset` — тоже клампит, но по-другому: см. `text.rs:76-90`).
- **РЕГРЕССИОННЫЙ ТЕСТ:** `property_position_roundtrip` — property-тест: для случайных строк
  (включая эмодзи, CJK, табы, `\r\n`, пустые строки, файл без финального `\n`) и случайных
  валидных `Position` выполняется `byte_offset(position_at(b)) == b` и
  `position_at(byte_offset(p)) == p`. Плюс `offset_of_and_byte_offset_agree` — таблица краевых
  случаев (`line == n_lines`, `character == 0`, `character == len`, `character == len+1`).
- **СЕРЬЁЗНОСТЬ:** medium.

---

## 7. Mux / несколько серверов на файл (наш `pa-lsp::mux`)

### F34 · `mux-identity-missing-init-options`
- **ИСТОЧНИК:** https://github.com/can1357/oh-my-pi/issues/15535 (CLOSED, PR #15645: «Mux idle reuse
  ignores initOptions: a new session gets a server initialized with old options»; наблюдали одинаковый
  PID и единственную запись `initialize` с `{"probe":"A"}`) и #8382 (CLOSED, PR #9226: «server cache
  keys collide across different LSP configurations»: «both identify a server only by executable
  command plus cwd»).
- **СИМПТОМ:** две сессии с разными `initializationOptions` получают один процесс; вторая сессия
  думает, что её настройки применены; `initialize` до сервера не доходит (отвечен из кеша).
- **ПРИЧИНА:** ключ идентичности не включал `initializationOptions`; `initialize` кешировался.
- **НАШ АНАЛИЗ:** `MuxIdentity::new` (`mux.rs:94-124`) хеширует `command, args, cwd, env` —
  **`initializationOptions` не входит**, и у нас вообще нет поля initOptions в `ServerConfig`
  (`servers.rs:11-25`: name/command/args/extensions/root_markers/language_id). Пока это не баг (нечего
  терять), но как только мы добавим `initOptions`/`settings` в конфиг сервера, mux начнёт отдавать
  чужой процесс — ровно как в #15535. Это **латентный** дефект: ключ должен включать всё, что влияет
  на процесс.
- **НАШ АНАЛОГ:** `pa-lsp::mux::MuxIdentity` (используется при `lsp.shared`), влияет на
  `lsp.status`, все `lsp.*` действия через общий сервер.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `mux_identity_covers_every_spawn_input` — параметризованный тест: изменение
  `command`/`args`/`cwd`/`env`/`init_options`/`language_id` меняет хеш; перестановка ключей `env` —
  нет (уже покрыто `identity_ignores_environment_order`, `mux.rs:439`). Сейчас кейс
  `init_options` даже не компилируется — тест должен появиться **вместе** с полем.
- **СЕРЬЁЗНОСТЬ:** high (латентный, сработает при первом же расширении конфига).

### F35 · `mux-per-identity-daemon-serializes-calls`
- **ИСТОЧНИК:** их mux (`mux/server.ts`) устроен иначе: один демон на проект, много серверов внутри,
  каждому линку свой процесс (`spawns one server per concurrent link`, тест
  `lsp-mux.test.ts:190`), с явной изоляцией id (`isolates equal request ids between sessions`) и
  оверлеев (`isolates open-document overlays between concurrent sessions`, #8371 → PR #8392).
- **СИМПТОМ:** параллельные LSP-запросы двух агентов сериализуются; один медленный запрос блокирует
  остальные.
- **ПРИЧИНА:** наш `Connection::call` (`mux.rs:261-277`) делает `write_frame` → `read_frame` и ждёт
  **следующий кадр вообще**, без сопоставления `id`. При двух одновременных `call` на одном
  `Connection` (или если демон пришлёт внеочередной кадр) ответы перепутаются/потеряются.
  `LspMux::daemon` (`mux.rs:326-338`) поднимает демон **на identity** (на сервер), а не на проект, так
  что два клиента с одинаковой identity делят один сокет и один процесс.
- **НАШ АНАЛИЗ:** `pa-lsp::mux` (сейчас не подключён к `LspManager` — `grep` показывает, что
  `LspMux` используется только в собственных тестах `mux.rs`; `LspManager` держит клиенты в процессе).
  То есть дефект «спящий», но код готов к использованию и небезопасен.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `mux_connection_matches_responses_by_id` — отправить два `call` параллельно
  (или вручную вклинить чужой кадр) и проверить, что каждый получил свой ответ.
  Плюс `mux_daemon_is_not_shared_across_workspaces` — две identity с разными `cwd` → разные сокеты.
- **СЕРЬЁЗНОСТЬ:** medium (латентный, но при включении — critical).

### F36 · `client-cache-lock-blocks-all-files`
- **ИСТОЧНИК:** https://github.com/can1357/oh-my-pi/issues/3001 (CLOSED: «Some of the LSP servers hang
  on start» — разбор про `clientLocks` и коллизию id), #8390 (CLOSED, PR #8523: «never reap a client
  with an in-flight request»), их `getOrCreateClient` (`client.ts:699-736`): ожидание чужого
  in-flight промиса, а не глобальный лок.
- **СИМПТОМ:** первый запрос к серверу на холодную блокирует **все** запросы ко **всем** файлам этого
  проекта, пока идёт handshake; зависший сервер вешает весь LSP-тул.
- **ПРИЧИНА:** грубая блокировка вместо «дождаться конкретного in-flight промиса».
- **НАШ АНАЛИЗ:** `LspManager::ensure_client` (`manager.rs:186-204`):
  ```rust
  let mut guard = self.clients.write().await;   // ← write-lock на всю карту
  ...
  match LspClient::spawn(server, &root).await { ... }   // ← spawn+initialize под локом
  ```
  `clients.read()` в `status()`/`clients()`/`any_client()` и `write()` в `shutdown_for`/`shutdown_all`
  ждут всё время handshake. При `initialize`, который не отвечает (сервер без `initialize` response),
  весь LSP мёртв — и, поскольку `_timeout` не применяется (F08), навсегда.
- **НАШ АНАЛОГ:** все `lsp.*` действия, `lsp.status`, `edit`-writethrough.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `cold_start_does_not_block_other_servers` — два сервера (`.rs` и `.py`),
  rust-analyzer «висит» на `initialize`. Вызвать `lsp.definition` для `.py`-файла.
  **Ожидаемо:** отвечает в пределах своего таймаута; `lsp.status` тоже отвечает (не блокируется).
  (Сейчас: оба ждут `clients.write()`.)
- **СЕРЬЁЗНОСТЬ:** critical.

---

## 8. Прочее, что прямо бьёт по нашим тулам

### F37 · `formatting-options-are-all-zero`
- **ИСТОЧНИК:** https://github.com/can1357/oh-my-pi/issues/2329 (CLOSED, PR #2330: «format-on-write
  hardcodes `tabSize: 3`, silently re-indents YAML…»). Их фикс — `format-options.ts`: precedence
  `.editorconfig` → sniff содержимого → fallback 2, плюс `TRIM_OPTIONS`.
- **СИМПТОМ:** format-on-write переписывает отступы всего файла (2→3), файл «успешно» сохранён, но
  диффом на весь файл.
- **ПРИЧИНА:** хардкод `FormattingOptions` вместо вывода из файла/editorconfig.
- **НАШ АНАЛИЗ:** `writethrough.rs:1006` (`client.rs:1006`):
  ```rust
  options: FormattingOptions::default(),
  ```
  `lsp_types::FormattingOptions` — `#[derive(Default)]` с `tab_size: u32` и `insert_spaces: bool`
  (`lsp-types-0.95.1/src/formatting.rs:41-51`), т.е. **`tabSize: 0, insertSpaces: false`**. Мы
  буквально просим форматтер «таб = 0 пробелов, табы». Для `yaml-language-server`/`prettier`
  это либо игнор, либо полное переформатирование в табы. Плюс `write_through` (`writethrough.rs:219-240`)
  пишет результат обратно на диск **без** восстановления `LineEnding`/BOM (в отличие от `pa-edit`,
  где `FileRead::persist` это делает) — CRLF-файл станет LF.
- **НАШ АНАЛОГ:** `edit`/`write` writethrough (`pa-lsp::writethrough`, `WriteThroughOptions::format_on_save`).
- **РЕГРЕССИОННЫЙ ТЕСТ:** `formatting_sends_detected_indent` — 2-space YAML, сервер-фейк записывает
  `FormattingOptions`; `write_through` с `format_on_save`.
  **Ожидаемо:** `tabSize == 2`, `insertSpaces == true` (из содержимого/`.editorconfig`), **не** 0/false.
  Плюс `formatting_preserves_crlf_and_bom`: после format-on-save CRLF-файл остаётся CRLF, BOM на месте.
- **СЕРЬЁЗНОСТЬ:** high.

### F38 · `code-action-context-empty`
- **ИСТОЧНИК:** https://github.com/can1357/oh-my-pi/issues/9358 (OPEN: «Automatically attach LSP
  suggestions/code actions to tool results, the same as diagnostics done») + их `tool.ts` code_actions:
  `diagnostics` берутся из `client.diagnostics.get(uri)` и, при reconcile, **дожидаются** свежих
  (`waitForDiagnostics`), чтобы quickfix-провайдер видел непустой `context.diagnostics`.
- **СИМПТОМ:** `code_actions` возвращает только рефакторинги; «Add missing import»/«Remove unused»
  отсутствуют, потому что `context.diagnostics` пуст.
- **ПРИЧИНА:** контекст не заполняется диагностиками.
- **НАШ АНАЛИЗ:** `client.rs:718`:
  ```rust
  context: lsp_types::CodeActionContext::default(),   // diagnostics: [], only: None
  ```
  Плюс `apply_code_action` (`tool.rs:939-951`) **игнорирует `action.command`** (возвращает `None` →
  инструмент печатает «(selected action has no edit to apply)») и **не вызывает
  `codeAction/resolve`** (у нас нет ветки resolve вообще), хотя сервер вправе вернуть action без
  `edit` и заполнить его по resolve.
- **НАШ АНАЛОГ:** `lsp.code_actions` (и `apply:true`).
- **РЕГРЕССИОННЫЙ ТЕСТ:** `code_actions_send_current_diagnostics` — файл с ошибкой; фейковый сервер
  возвращает quickfix только если `context.diagnostics` непуст.
  **Ожидаемо:** quickfix в списке (сейчас — нет).
  Плюс `code_action_resolve_is_called` — action без `edit`, resolve отдаёт `edit` → применяется.
  Плюс `command_only_code_action_is_reported_not_silently_skipped`.
- **СЕРЬЁЗНОСТЬ:** high (половина возможностей code actions недоступна).

### F39 · `reload-erases-configuration`
- **ИСТОЧНИК:** https://github.com/can1357/oh-my-pi/issues/8383 (CLOSED, PR #9228: «generic reload sends
  empty settings instead of active LSP config»: `{}` — валидный LSP, но означает «конфигурация теперь
  пуста», и сервер молча съезжает на дефолты), #8571 (CLOSED, PR #8572: `rust-analyzer/reloadWorkspace`
  слался **всем** серверам и ронял Roslyn, `dotnet/roslyn#84890`), #6369 (CLOSED, PR #6403:
  отменённый reload убивал сервер и рапортовал success).
- **СИМПТОМ:** `lsp reload` «успешно» перезагружает, но сервер теряет настройки; или падает целиком.
- **ПРИЧИНА:** (а) пустой `settings`; (б) rust-анализаторный метод всем подряд; (в) отмена
  трактуется как «метод не поддержан».
- **НАШ АНАЛИЗ:** `client.rs:803-828`:
  ```rust
  pub async fn reload(&self) -> anyhow::Result<()> {
      if self.name == "rust-analyzer" { let _ = self.server.request::<RustAnalyzerReloadWorkspace>(...).await; }
      ...
      .did_change_configuration(DidChangeConfigurationParams { settings: serde_json::Value::Null })
  ```
  Мы шлём **`null`** (хуже, чем `{}`: часть серверов падает на `null` — ровно кейс #5745
  «roslyn crashes after initialized — workspace/configuration answered with {} instead of null», и
  обратный: `null` вместо `{}`), и у нас **нет** `config.settings` вообще (`ServerConfig` без
  settings). Плюс `lsp.reload` (`tool.rs:213-221`) **не перезапускает** процесс, хотя
  документирован как «reload»; и `LspManager::restart` (`manager.rs:251-260`) существует, но нигде
  не вызывается.
- **НАШ АНАЛОГ:** `lsp.reload`.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `reload_sends_configured_settings` — `ServerConfig` с `settings: {"x":1}`;
  `lsp.reload` → фейковый сервер видит `didChangeConfiguration{settings:{x:1}}` (не `null`).
  Плюс `reload_does_not_send_rust_analyzer_method_to_other_servers` — gopls-клиент не получает
  `rust-analyzer/reloadWorkspace`.
  Плюс `reload_reports_whether_the_process_restarted` — текст говорит «refreshed», если процесс жив.
- **СЕРЬЁЗНОСТЬ:** high.

### F40 · `approval-gate-absent`
- **ИСТОЧНИК:** их `LspTool.approval` (`tool.ts:130-141`): `LSP_READONLY_ACTIONS` → `"read"`, иначе
  `"write"`; `formatApprovalDetails` печатает action+file.
- **СИМПТОМ:** мутирующее действие проходит как read-only (или наоборот).
- **ПРИЧИНА:** классификация не связана с реальным гейтом.
- **НАШ АНАЛИЗ:** `Action::is_readonly` (`types.rs:69-82`) **корректно** относит `Rename`,
  `CodeActions`, `ReplaceSymbolBody`, `Insert*`, `SafeDeleteSymbol`, `Reload`, `RenameFile` к write —
  но метод **не используется нигде** (`grep is_readonly` → только определение и его собственный тест).
  В `LspTool::execute` нет ни проверки plan-mode, ни approval; `edit_hybrid.rs:165` вообще
  хардкодит `plan_active: false`, т.е. plan-mode не блокирует и `edit`.
- **НАШ АНАЛОГ:** `lsp.rename` / `lsp.rename_file` / `lsp.safe_delete_symbol` /
  `lsp.code_actions(apply)` / `lsp.replace_symbol_body` и `edit`.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `write_actions_require_approval` — в plan-mode/read-only сессии
  `lsp.rename{apply:true}` → отказ; `lsp.references` → разрешён.
  Плюс `is_readonly_matches_actual_mutation` — таблица: для каждого `Action` вызвать на фейковом
  сервере и проверить, что `is_readonly() == false` ⇔ на диске что-то изменилось.
- **СЕРЬЁЗНОСТЬ:** high (безопасность/политика).

---

## ТОП-10 тестов, которые надо написать первыми

Отсортировано по «цена ошибки × вероятность».

1. **`workspace_edit_refuses_before_any_write`** (F03) — `WorkspaceEdit` с `changes` + `Operations`
   не должен изменить ни байта. Сейчас `bail!` приходит **после** записи. Это самый дешёвый тест с
   самым большим выигрышем: он же ловит всю неатомарность `apply_workspace_edit`.
2. **`rename_file_never_overwrites_existing_destination`** (F05) — `fs::rename` на POSIX молча
   уничтожает существующий файл. Одна строка теста, необратимая потеря данных.
3. **`safe_delete_refuses_when_references_unknown`** (F06) — fail-open на необратимой операции:
   `Err(_) => Vec::new()` → символ удалён. Тест на фейковом сервере, который просто отвечает ошибкой.
4. **`rename_file_aborts_when_willrename_fails`** (F04) — `if let Ok(Some(edit))` глотает всё:
   move происходит без правки импортов, инструмент рапортует success.
5. **`publish_during_read_is_not_lost`** (F13) — `try_lock` теряет публикацию → недетерминированный
   ложный «clean». Тест детерминированный (захватить мьютекс), ловит худший класс ошибки —
   «диагностик нет» на файле с ошибками.
6. **`every_action_honors_timeout`** (F08) — `_timeout` не используется в 10 действиях; зависший
   сервер вешает агент навсегда. Тест — фейковый сервер, который не отвечает.
7. **`edit_beyond_eof_is_rejected`** (F31) — `offset_of` клампит, `ensure!` проходит, правка
   дописывается в конец файла. Ловит весь класс «позиция вне файла» в rename/code_actions/rename_file.
8. **`same_symbol_name_in_two_languages_stays_two_nodes`** (F25) — `UNIQUE(project, qualified_name)` +
   `upsert` перезаписывает `file_path`: символ молча исчезает из своего файла, рёбра уезжают.
   Ломает весь `graph.*`.
9. **`multi_file_edit_is_all_or_nothing`** (F19) — `session.rs` прямо документирует
   «files already written stay written»; для hashline это ещё и рассинхрон снапшотов.
10. **`hashline_range_splitting_a_node_is_rejected`** (F20) — у нас `parses_cleanly` даёт только
    warning, файл остаётся битым (ровно как в #15119). Плюс `line_zero_is_rejected_not_panicking`
    (F11) — как бонус-тест в том же файле, потому что это panic.

**Следующая пятёрка** (когда эти зелёные): `formatting_sends_detected_indent` (F37),
`incremental_reindex_refreshes_coverage` (F26), `ambiguous_call_does_not_create_a_cycle` (F27),
`code_actions_send_current_diagnostics` (F38), `diagnostics_match_reencoded_uri` (F16).

---

## Приложение: что у oh-my-pi сделано лучше нас (стоит портировать)

| Их механизм | Где | Зачем нам |
|---|---|---|
| `waitForDiagnostics` как чистый state machine с mock-часами | `diagnostics.ts:208-274` | у нас `DiagnosticsWaiter` есть (`client.rs:92-150`), но не подключён к `$/progress` |
| `EquivalentUriMap` | `utils.ts:76-101` | F16 |
| `sortAndValidateTextEdits` + дедуп байт-идентичных правок + отказ от snippet | `edits.ts:92-150`, PR #4460/#8394 | у нас нет ни дедупа, ни overlap-проверки в `apply_text_edits` |
| `applyEditsThenRename` со снапшотами и откатом | `edits.ts:187-215`, PR #8395 | F05-соседний |
| `reconcileFileFromDisk` перед каждой семантической операцией | `client.ts:1000-1100`, PR #11419 | F01 |
| `notifyWorkspaceWatchedFiles` + `refreshFile` всех открытых документов | `client.ts:1560-1600`, PR #4462/#12857 | F17 |
| `shutdownStaleClients` по identity конфига | `client.ts:1050-1130`, PR #9636 | F34 |
| `MAX_RENAME_PAIRS` + `enumerateRenamePairs` | `tool.ts:88-135` | F05 |
| `references` с ретраями и `isOnlyQueriedDeclaration` | `tool.ts:1290-1330` | F09 |
| их тест-файл `lsp-regressions.test.ts` (~180 кейсов) | `test/tools/` | готовый чеклист для нас |
