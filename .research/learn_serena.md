# Serena → наш аудит: баги, краевые случаи, инварианты (для регрессионных тестов)

**Источник:** `oraios/serena`, локальный read-only клон `/tmp/serena` @ `d9e4c79` (2026-10-15),
+ GitHub issues/PRs (`gh issue list -R oraios/serena --limit 300 --state all`, `gh pr list`).
Номера в скобках — реальные issue/PR; `файл:строка` — проверено в клоне на этом коммите.

**Легенда статусов в источниках:** `[fixed]` — починено в апстриме (но тест всё равно нужен нам),
`[open]` — открыто в апстриме (значит класс бага живой), `[by-design]` — поведение признано ограничением.

Формат каждой находки: ID / ИСТОЧНИК / СИМПТОМ / ПРИЧИНА / НАШ АНАЛОГ / РЕГРЕССИОННЫЙ ТЕСТ / СЕРЬЁЗНОСТЬ.

---

## A. rename / rename_file / safe_delete

### A1. `rename-silent-partial-edit`
- **ИСТОЧНИК:** issue #1744 `[open]` — https://github.com/oraios/serena/issues/1744 ;
  код: `src/solidlsp/ls.py:3262-3263` (`request_rename_symbol_edit` открывает только целевой файл),
  `src/serena/code_editor.py:435-443` (успех = `num_changes > 0`).
- **СИМПТОМ:** `rename_symbol` на Zig-проекте вернул `WorkspaceEdit` только для целевого файла
  (ZLS отдаёт правки лишь для открытых файлов). Serena применила что было, вернула
  `Successfully renamed ... (1 changes applied)`, проект перестал компилироваться — «успешный» ренейм
  оставил висеть ссылки в других файлах.
- **ПРИЧИНА:** клиент не отличает «сервер вернул полный набор правок» от «сервер вернул то, что успел».
  Никакой сверки с `textDocument/references` перед применением нет; `num_changes` считает
  *операции*, а не *места*, и не сравнивается с числом известных ссылок.
- **НАШ АНАЛОГ:** `lsp.rename` (и любой `rename` через `edit.apply_patch` по WorkspaceEdit).
- **РЕГРЕССИОННЫЙ ТЕСТ:** файл `a.ts` c `export const f`; `b.ts` и `c.ts` импортируют и вызывают `f`.
  LSP-двойник отвечает на `textDocument/rename` правкой **только** для `a.ts`.
  Ожидание: `lsp.rename` возвращает **ошибку/предупреждение** вида
  `partial rename: 2 known references in b.ts, c.ts not covered`, и **не** пишет файлы
  (или пишет, но с явным флагом `partial=true` + список непокрытых файлов). Дополнительно:
  после успешного полного ренейма `lsp.references` не находит старое имя.
- **СЕРЬЁЗНОСТЬ:** critical (тихая порча кода + ложный успех).

### A2. `workspace-edit-applies-changes-AND-documentChanges`
- **ИСТОЧНИК:** код: `src/serena/code_editor.py:376-395` — два независимых `if`
  (`if "changes" in workspace_edit:` затем `if "documentChanges" in workspace_edit:`);
  спецификация в самом репо: `src/solidlsp/lsp_protocol_handler/lsp_types.py:1347-1352`
  («The edit should **either** provide `changes` **or** `documentChanges`. If documentChanges are
  present they are **preferred** over `changes`»).
- **СИМПТОМ:** если сервер (или наш собственный генератор правок) заполнил оба поля, правки применяются
  дважды: дублируются вставки, «двойной» ренейм даёт `foo → foo → foo`-подобные артефакты,
  а `num_changes` завышен (=> A6 маскируется).
- **ПРИЧИНА:** реализовано «применить всё, что нашли», вместо «выбрать один источник правды».
- **НАШ АНАЛОГ:** `lsp.rename`, `edit.apply_patch` (если он принимает WorkspaceEdit), `lsp.rename_file`.
- **РЕГРЕГСCИОННЫЙ ТЕСТ:** юнит на нормализатор WorkspaceEdit: вход
  `{"changes": {uri: [edit]}, "documentChanges": [{"textDocument": {...}, "edits": [тот же edit]}]}` →
  ожидание: применена **одна** правка, `num_changes == 1`, содержимое файла изменено один раз.
  Второй кейс: `changes` пустой, `documentChanges` заполнен → применяется `documentChanges`.
- **СЕРЬЁЗНОСТЬ:** high.

### A3. `rename-unhandled-documentchange-kinds`
- **ИСТОЧНИК:** `src/serena/code_editor.py:387-393`: обрабатывается только `kind == "rename"`,
  иначе `raise ValueError("Unhandled document change kind ... Please report to Serena developers.")`.
  LSP допускает `create` / `rename` / `delete` (`lsp_types.py:3221+`, `WorkspaceEdit` docstring 1347-1358).
- **СИМПТОМ:** ренейм, при котором сервер создаёт файл (перенос символа в новый модуль, `create` +
  `TextDocumentEdit` в этот файл), падает целиком с внутренней ошибкой «report to developers»;
  агент получает непонятную ошибку и повторяет вызов.
- **ПРИЧИНА:** неполная реализация `documentChanges`; отсутствие «частичного» режима
  (применить текст-едиты, ресурс-операции — отдельно/по порядку).
- **НАШ АНАЛОГ:** `lsp.rename`, `lsp.rename_file`, `edit.apply_patch`.
- **РЕГРЕССИОННЫЙ ТЕСТ:** WorkspaceEdit c `documentChanges = [CreateFile(new.ts), TextDocumentEdit(new.ts, [...])]` →
  ожидание: либо (а) обе операции применены в порядке следования, либо (б) **отказ до записи на диск**
  с сообщением `unsupported document change kind: create` и файл(ы) не изменены (проверять `git status` чист).
- **СЕРЬЁЗНОСТЬ:** medium.

### A4. `rename-edit-uri-escapes-project-root`
- **ИСТОЧНИК:** `src/serena/code_editor.py:346-347` (`_relative_path_from_uri` = `os.path.relpath(uri_to_path(uri), project_root)`),
  `352-374` (`EditOperationFileTextEdits` → `edited_file_context`; `EditOperationRenameFile` → `os.rename`).
  Прямое следствие: путь из ответа сервера **не** проходит `validate_relative_path` (в отличие от
  пользовательского `relative_path`, см. CHANGELOG «the line-based and symbol-level editing tools did not
  apply the path-confinement check at all and now validate the file path», строка 125 CHANGELOG).
- **СИМПТОМ:** WorkspaceEdit с URI `file:///etc/foo` или `file:///home/u/other-repo/x.py` даёт
  `relative_path = ../../...`, который затем открывается и **записывается** (relpath не запрещает выход
  за корень). Т.е. «доверяем ответу LS» = запись вне проекта, включая `os.rename` произвольных путей.
- **ПРИЧИНА:** валидация пути сделана **на входе инструмента** (`edit_api.py:214/230/248…` вызывает
  `validate_relative_path` для пользовательского `relative_path`), но **не на выходе LSP-ответа**:
  `edited_file_context` (`code_editor.py:83-92`) проверяет только `FileProxy.is_external_path`
  (для LSP-бэкенда всегда `False`, `lsp_backend.py:74-76`), а `_save_edited_file` (`code_editor.py:94-97`)
  делает `os.path.join(self.project_root, relative_path)` + `write_file_atomic` — без `is_path_in_project`.
- **НАШ АНАЛОГ:** `lsp.rename` / `lsp.rename_file` / `edit.apply_patch` (любой путь, приходящий от LSP).
- **РЕГРЕССИОННЫЙ ТЕСТ:** mock-LSP возвращает `WorkspaceEdit` с правкой для URI вне project root →
  ожидание: операция отклонена (`path outside project root`), файл вне корня не создан/не изменён,
  внутрипроектные правки тоже не применены (атомарный отказ).
- **СЕРЬЁЗНОСТЬ:** high (безопасность).

### A5. `rename-file-no-willRenameFiles`
- **ИСТОЧНИК:** `src/serena/code_editor.py:365-374` (`EditOperationRenameFile.apply` = голый `os.rename`);
  `workspace/willRenameFiles` объявлен, но **никем не вызывается**:
  `src/solidlsp/ls_request.py:128-134`, `src/solidlsp/lsp_protocol_handler/lsp_requests.py:160-166`
  (grep по `src/` не находит ни одного вызова, кроме определения); `didRenameFiles` — только определение
  (`lsp_requests.py:448`).
- **СИМПТОМ (из экосистемы, подтверждено CHANGELOG строка ~256):** pyrefly 1.1.1 «advertises
  `workspace/willRenameFiles` but answers it with `null`; 1.2.0 answers with the import edits …
  the two absolute importers of a renamed module». То есть **импорты обновляет именно willRenameFiles**,
  а не `textDocument/rename`. При переносе файла без этого запроса импорты остаются указывать на старый путь.
  Дополнительно LS не получает `didRenameFiles` → держит файл открытым под старым URI → последующие
  `references`/`diagnostics` отвечают по несуществующему пути.
- **ПРИЧИНА:** файловый ренейм реализован как операция ФС, а не как LSP-протокол
  (`willRenameFiles` → правки импортов → `os.rename` → `didRenameFiles`).
- **НАШ АНАЛОГ:** `lsp.rename_file` (наш отдельный тул!) — это **прямой** аналог, самый вероятный носитель бага.
- **РЕГРЕССИОННЫЙ ТЕСТ:** проект `pkg/models.py` (класс `M`), `app.py: from pkg.models import M`,
  `pkg/__init__.py: from .models import M`. Вызов `lsp.rename_file("pkg/models.py", "pkg/domain.py")`:
  ожидание — файл переименован **и** (при поддержке LS) импорты в `app.py`/`__init__.py` обновлены;
  если LS не поддерживает `willRenameFiles` — тул возвращает явное предупреждение
  `imports not updated: 2 files reference the old path` + список, а не тихий успех.
  Плюс тест: после `rename_file` `lsp.diagnostics("app.py")` не содержит unresolved-import.
- **СЕРЬЁЗНОСТЬ:** high.

### A6. `rename-counts-operations-as-success`
- **ИСТОЧНИК:** `src/serena/code_editor.py:397-407` (`_apply_workspace_edit` возвращает `len(operations)`),
  `435-443` (порог `num_changes == 0`).
- **СИМПТОМ:** ренейм, который вернул правки только для «пустых» мест (или правки с `newText == oldText`,
  т.е. no-op), рапортуется как `(1 changes applied)`. Обратный случай: сервер вернул 1 операцию на 5 файлов —
  сообщение «1 changes applied» вводит агента в заблуждение о полноте.
- **ПРИЧИНА:** метрика «сколько операций применили», а не «сколько мест реально изменилось» и не «покрыты ли все ссылки».
- **НАШ АНАЛОГ:** `lsp.rename`, `edit.replace`, `edit.patch` (сообщение об успехе).
- **РЕГРЕССИОННЫЙ ТЕСТ:** LSP возвращает один `TextEdit` с `newText == oldText` →
  ожидание: результат `no-op rename (0 identifiers changed)`, статус — предупреждение, а не `OK`.
  Второй кейс: 3 файла, 7 мест → сообщение содержит `7 edits in 3 files`.
- **СЕРЬЁЗНОСТЬ:** medium.

### A7. `safe-delete-trusts-empty-references`
- **ИСТОЧНИК:** `src/serena/repl/api/lsp_api.py:768-800` (`safe_delete_symbol`): единственная проверка —
  `references_locations = lang_server.request_references(...)`; `if file_to_lines: return "Cannot delete..."`;
  иначе `delete_symbol(...)`.
- **СИМПТОМ:** на холодном/недоиндексированном сервере (или после рестарта LS, или при таймауте references,
  который был «съеден» и вернул `[]`) `safe_delete` **удаляет символ с реальными вызовами**. Аналог #1814:
  tsserver умер → `find_referencing_symbols` вернул `{}` за 0.8 s, «cross-file indexing complete», `isError: false`.
- **ПРИЧИНА:** пустой список ссылок трактуется как доказательство отсутствия ссылок; нет ни проверки
  «индекс готов», ни перекрёстной проверки текстовым поиском имени по проекту, ни требования
  явного подтверждения при непустом текстовом совпадении.
- **НАШ АНАЛОГ:** `lsp.safe_delete_symbol` — **самый опасный** из наших тулов (необратимо удаляет).
- **РЕГРЕССИОННЫЙ ТЕСТ:** (1) mock-LSP: `references → []`, но в проекте есть файл с текстом `target_symbol(` →
  ожидание: удаление **запрещено**, ответ `references not confirmed (index may be incomplete); 1 textual
  occurrence found in x.py`. (2) mock-LSP: `references` бросает таймаут → удаление запрещено, ошибка
  пробрасывается. (3) честный кейс: `references → []` и текстовых вхождений нет → удаление разрешено.
- **СЕРЬЁЗНОСТЬ:** critical.

### A8. `safe-delete-assert-instead-of-error`
- **ИСТОЧНИК:** `src/serena/repl/api/lsp_api.py:783-784`
  (`assert symbol_rel_path == relative_path, f"Symbol ... is not in the expected relative path ..."`),
  плюс `790-792` (`assert symbol_line is not None ...`).
- **СИМПТОМ:** если `name_path_pattern` сматчился в другом файле (или символ без `selectionRange`),
  пользователь получает `AssertionError` вместо внятной ошибки; при `python -O` проверки исчезают вовсе
  (и тогда `delete_symbol` идёт по чужому пути). Issue #1924 показывает, что апстрим такие `assert`
  в tool-параметрах считает не-багом («the model can handle AssertionError»), т.е. это **живой** класс.
- **ПРИЧИНА:** инварианты выражены `assert` в коде, достижимом из пользовательского ввода.
- **НАШ АНАЛОГ:** `lsp.safe_delete_symbol`, `lsp.rename`, `lsp.replace_symbol_body` (валидация «символ в этом файле»).
- **РЕГРЕССИОННЫЙ ТЕСТ:** `safe_delete_symbol(name_path="foo", relative_path="b.py")`, где `foo` есть только в `a.py` →
  ожидание: структурированная ошибка `symbol 'foo' not found in b.py (found in a.py)`; тип — доменная ошибка,
  не `AssertionError`; под `python -O` поведение то же (тест гонять с `-O`).
- **СЕРЬЁЗНОСТЬ:** low (но дешёвый тест, и `-O`-режим делает его high).

### A9. `safe-delete-ignores-imports-and-dynamic-refs`
- **ИСТОЧНИК:** `src/serena/repl/api/lsp_api.py:794-800` — в отчёт идут **все** ссылки как строки,
  но решение «нельзя» принимается только по непустому списку; `request_references` вызывается с
  `context.includeDeclaration = False` (`src/solidlsp/ls.py:1785`), а `find_referencing_symbols` в
  остальных местах фильтрует импорты (`include_imports=False`, `symbol.py:868`).
- **СИМПТОМ:** для символа, который «используется» только через реэкспорт (`__init__.py: from .x import Y`),
  часть серверов не считает импорт ссылкой → `safe_delete` удаляет `Y`, ломая публичный API пакета.
  Обратный случай: символ, используемый через `getattr(module, "Y")` / DI-контейнер / строковый роутинг,
  удаляется как «без ссылок» — это не лечится LSP в принципе.
- **ПРИЧИНА:** «безопасно» определяется только LSP-ссылками; нет ни анализа реэкспортов, ни предупреждения
  о динамических обращениях.
- **НАШ АНАЛОГ:** `lsp.safe_delete_symbol`.
- **РЕГРЕССИОННЫЙ ТЕСТ:** `pkg/mod.py: class Y`, `pkg/__init__.py: from .mod import Y`,
  никаких других использований. `safe_delete_symbol("Y", "pkg/mod.py")` →
  ожидание: отказ или предупреждение `Y is re-exported by pkg/__init__.py` (не молчаливое удаление).
- **СЕРЬЁЗНОСТЬ:** high.

### A10. `safe-delete-whitespace-heuristic`
- **ИСТОЧНИК:** issue #1697 `[fixed, by-design-хейзик]` — https://github.com/oraios/serena/issues/1697 ;
  код: `src/serena/code_editor.py:266-297` (после удаления тела сканирует последующие пустые строки и удаляет их);
  комментарий мейнтейнера в issue: «This could only ever be a heuristic … I am not sure that's an important
  issue to solve on our side».
- **СИМПТОМ:** `insert_after_symbol` + `safe_delete_symbol` не возвращают файл байт-в-байт
  (остаются лишние пустые строки). В отчёте issue — `'return 1\n\n\n' → 'return 1\n\n\n\n'`.
  После фикса PR #1703 добавлен регрессионный тест, но он покрывает **одну** форму вставки
  (`test_insert_and_delete_no_change`, `test/serena/test_symbol_editing.py:589-601`).
- **ПРИЧИНА:** удаление «съедает» пустые строки **после** символа, но не нормализует разделитель;
  результат зависит от того, сколько пустых строк было до/после и от типа символа
  (`is_neighbouring_definition_separated_by_empty_line`, `symbol.py:258-259`).
- **НАШ АНАЛОГ:** `lsp.safe_delete_symbol` (и `lsp.insert_before_symbol`/`insert_after_symbol` — общая эвристика отступов).
- **РЕГРЕССИОННЫЙ ТЕСТ:** property-тест: для N ∈ {0,1,2} пустых строк до и после, для body с/без trailing `\n`,
  для символа в начале/середине/конце файла: `insert_after` затем `safe_delete` →
  содержимое файла **идентично** исходному (сравнение байтов, включая CRLF-вариант).
- **СЕРЬЁЗНОСТЬ:** medium (накопительный шум в diff'ах, ломает «чистые» PR).

---

## B. references / definitions / implementation

### B1. `refs-same-name-caller-dropped-as-import`
- **ИСТОЧНИК:** issue #2133 `[fixed]` — https://github.com/oraios/serena/issues/2133 ; PR #2134;
  код до фикса: `src/solidlsp/ls.py:2628-2648` (проверка «это импорт» = совпало имя+kind и `incoming_symbol` уже виден),
  фикс добавляет проверку попадания `ref` в `selectionRange` содержащего символа.
- **СИМПТОМ:** Gleam: `b.run` вызывается из `a.run` и `c.run`; `find_referencing_symbols("run")` вернул только `a.run`.
  LSP напрямую отдаёт оба. То есть **часть вызывающих теряется**, если у них то же имя, что у цели,
  и LS перечислил их после объявления цели.
- **ПРИЧИНА:** эвристика «импорт vs вызов» опиралась только на совпадение имени/kind и порядок элементов,
  без проверки геометрии (лежит ли ссылка внутри *имени* символа).
- **НАШ АНАЛОГ:** `lsp.references` / `lsp.find_referencing_symbols` — любой пост-фильтр импортов у нас.
- **РЕГРЕССИОННЫЙ ТЕСТ:** три файла: `target.py: def run()`, `first.py: def run(): target.run()`,
  `second.py: def run(): target.run()`; scripted-LSP отдаёт ссылки в порядке
  [decl(target), call(first), call(second)] → ожидание: 2 вызывающих (`first.run`, `second.run`).
  **Плюс инвариант:** `len(refs) == len(ссылки, которые вернул LSP) − (импорты + self)` — т.е. ни одна
  не-импортная ссылка не отброшена.
- **СЕРЬЁЗНОСТЬ:** high (тихая неполнота → агент удаляет «мёртвый» код).

### B2. `refs-silently-partial-while-indexing`
- **ИСТОЧНИК:** issues #1937 `[fixed]`, #1858 `[fixed]`, #1681 `[fixed]`, #1814 `[fixed]`:
  https://github.com/oraios/serena/issues/1937 (TypeScript: «waits only before the FIRST cross-file query»,
  таблица 11 → 1 → 11 файлов), #1858 (Scala: фикс. 5 s вместо ожидания Metals), #1681 (pyright: 5 s cap),
  #1814 (tsserver OOM → `{}` + «indexing complete»);
  код: `src/solidlsp/language_servers/typescript_language_server.py:635-660` (latch `_has_waited_for_cross_file_references`),
  `src/solidlsp/ls.py:1736-1749`, `ls.py:1573-1581`.
- **СИМПТОМ:** `find_referencing_symbols` возвращает **подмножество** ссылок без ошибки и без признака неполноты;
  повторный вызов через 2 s даёт полный ответ. Для агента «1 файл» и «11 файлов» неразличимы.
- **ПРИЧИНА:** готовность индекса определяется один раз (latch) и/или фиксированным таймаутом; ответ LS
  не несёт признака «индекс ещё строится»; `$/progress` не дожидается.
- **НАШ АНАЛОГ:** `lsp.references`, `lsp.implementation`, `graph.trace` (все, кто читает кросс-файловый индекс).
- **РЕГРЕССИОННЫЙ ТЕСТ:** scripted-LSP: первый вызов `references` возвращает 1 локацию, второй (через 100 мс) — 11;
  индекс «готов» только после 3-го `$/progress end`. Ожидание: либо тул ждёт готовности и возвращает 11,
  либо возвращает 1 **с явным флагом** `partial: true, index_state: "indexing"` — но **никогда** просто `1`
  без маркера. Проверять на всех тулах, читающих индекс.
- **СЕРЬЁЗНОСТЬ:** critical (самый частый «тихий» класс в Serena — 4 issue).

### B3. `refs-empty-vs-crash-indistinguishable`
- **ИСТОЧНИК:** issue #1814 `[fixed]` — https://github.com/oraios/serena/issues/1814 ;
  `src/solidlsp/language_servers/typescript_language_server.py:646` (`self._raise_if_crashed()` **до** latch —
  комментарий: «a crash observed after the first query's wait would otherwise never be raised»).
- **СИМПТОМ:** tsserver упал по OOM; Serena залогировала `TypeScript cross-file indexing complete`,
  вернула `Result: {}`, `isError: false`. Все последующие вызовы — `{}` за 0.8 s.
- **ПРИЧИНА:** падение процесса LS не превращалось в исключение на пути чтения результата; проверка
  «жив ли сервер» стояла только перед первым запросом.
- **НАШ АНАЛОГ:** `lsp.*` (любой), `graph.status`/`graph.coverage` (они *обязаны* показывать «индекс мёртв»).
- **РЕГРЕССИОННЫЙ ТЕСТ:** mock-LSP: `references` бросает `LanguageServerTerminatedException` на 2-м вызове →
  ожидание: (а) тул возвращает ошибку с `language_server_terminated`, (б) `graph.status` после этого
  показывает `index: unavailable`, а **не** `complete`, (в) `graph.coverage` не возвращает 100 %.
- **СЕРЬЁЗНОСТЬ:** high.

### B4. `refs-outside-root-silently-dropped`
- **ИСТОЧНИК:** `src/solidlsp/ls.py:1631-1653` (`convert_location_item`): `rel_path_str is None` →
  `log.warning("... probably the LS is parsing things in installed packages or in the standardlib! ...
  This is a bug but we currently simply skip these locations.")` → `return None`.
- **СИМПТОМ:** ссылки в путях вне корня (stdlib, installed packages, соседний пакет монорепо, symlink-цель)
  **исчезают** из результата; в тул-ответе нет ни счётчика, ни пометки.
- **ПРИЧИНА:** «пропускаем молча», решение задокументировано как «we currently simply skip».
- **НАШ АНАЛОГ:** `lsp.references`/`definition`/`declaration`; `graph.search` (внешние узлы).
- **РЕГРЕССИОННЫЙ ТЕСТ:** LSP отдаёт 3 локации: 2 внутри проекта, 1 в `/usr/lib/python3/x.py` →
  ожидание: результат `{refs: [2], skipped_outside_project: 1}`; при `include_external=false`
  всё равно есть счётчик `skipped: 1`.
- **СЕРЬЁЗНОСТЬ:** medium.

### B5. `declaration-in-archive-dropped-as-nonexistent`
- **ИСТОЧНИК:** issue #2099 `[open]` — https://github.com/oraios/serena/issues/2099 ;
  код: `src/solidlsp/ls.py:1643-1647` (`if not os.path.exists(abs_path): log.info("... non-existent path ..."); return None`).
- **СИМПТОМ:** Kotlin: `find_declaration` на вызове функции из зависимости с attached sources-jar →
  LSP корректно резолвит в `...sources.jar!/pkg/Foo.kt`, Serena выбрасывает локацию как «несуществующий путь» →
  `ValueError: No symbol declaration found at the location of the regex match`.
- **ПРИЧИНА:** фильтр «файл должен существовать на диске» применяется и к виртуальным/архивным URI.
- **НАШ АНАЛОГ:** `lsp.definition` / `lsp.declaration` / `lsp.type_definition` (все через общий `convert_location`).
- **РЕГРЕССИОННЫЙ ТЕСТ:** LSP возвращает локацию с URI `file:///x.jar!/a.kt` (или несуществующим путём внутри
  архива) → ожидание: локация возвращается как «external/archive» (path + `in_archive: true`),
  а не отбрасывается; `find_declaration` возвращает символ или явную ошибку `declaration is in an archive`,
  но не «not found».
- **СЕРЬЁЗНОСТЬ:** high.

### B6. `containing-symbol-hack-misattributes-reference`
- **ИСТОЧНИК:** `src/solidlsp/ls.py:2555-2580` — комментарий в коде:
  `# TODO: HORRIBLE HACK! I don't know how to do it better for now... / # THIS IS BOUND TO BREAK IN MANY CASES!
  IT IS ALSO SPECIFIC TO PYTHON!`; эвристика: `ref_text = file_data.contents.split("\n")[ref_line]`,
  `containing_symbol_name = ref_text.split(".")[0]`, поиск символа с этим именем и `kind == Variable`.
- **СИМПТОМ:** ссылка в строке, где нет «контейнера» (присваивание атрибута, комментарий, строка, multiline-выражение),
  приписывается **произвольному** символу-переменной с именем = текст до первой точки. Итог: в
  `find_referencing_symbols` появляется символ, который на самом деле не ссылается, или (при `include_file_symbols`)
  синтетический File-символ, которого нет в коде.
- **ПРИЧИНА:** отсутствие «containing symbol» на строке подменяется текстовой эвристикой вместо отказа.
- **НАШ АНАЛОГ:** `lsp.references` (группировка по содержащему символу), `graph.trace` (рёбра «кто вызывает кого»).
- **РЕГРЕССИОННЫЙ ТЕСТ:** ссылка на строке `obj.attr = value` внутри функции `f`, при наличии
  модульной переменной `obj` → ожидание: содержащий символ = `f` (или `null` + `unresolved: true`),
  **никогда** не `obj` и не синтетический файл. Второй кейс: ссылка внутри строкового литерала →
  `unresolved`, а не «файл целиком».
- **СЕРЬЁЗНОСТЬ:** high.

### B7. `implementations-empty-for-class`
- **ИСТОЧНИК:** issue #2019 `[open]` — https://github.com/oraios/serena/issues/2019 .
- **СИМПТОМ:** `find_implementations` для Python-класса возвращает `[]` под pyrefly/pyright (pyright вообще
  `MethodNotFound` на `textDocument/implementation`), хотя подтипы существуют и
  `prepareTypeHierarchy`/`typeHierarchy/subtypes` отвечают за ~1 s.
- **ПРИЧИНА:** `[]` от сервера трактуется как «нет реализаций», а не как «сервер это не умеет»;
  нет fallback на type hierarchy и нет различения «not supported» / «none found».
- **НАШ АНАЛОГ:** `lsp.implementation` / `lsp.find_implementations`; `graph.architecture` (наследники).
- **РЕГРЕССИОННЫЙ ТЕСТ:** mock-LSP: `textDocument/implementation` → `[]` для kind=Class,
  `typeHierarchy/subtypes` → 2 класса → ожидание: 2 результата (fallback) **или** явный
  `unsupported: true, reason: "server does not implement implementation for classes"`.
  Отдельный кейс: сервер отвечает `MethodNotFound` → `unsupported`, не ошибка, не `[]`.
- **СЕРЬЁЗНОСТЬ:** medium.

### B8. `multi-ls-same-file-two-answers`
- **ИСТОЧНИК:** `src/serena/ls_manager.py:175-191` (`_get_suitable_language_server`: **первый** LS в порядке
  словаря, который не игнорирует файл; `get_language_server` при `len(servers) > 1`), `99-102`
  (`_default_language_server` = `next(iter(...))`); `src/serena/symbol.py:751-768` (`find` без
  `within_relative_path` итерирует **все** LS).
- **СИМПТОМ:** при нескольких LS на один файл (vue/svelte + companion TS, angular + TS, TS + json-LS)
  (а) выбор сервера = «первый в порядке конфига, который не игнорирует файл», т.е. зависит от порядка
  `languages:` в `project.yml`, а не от того, кто реально владеет файлом (перестановка двух языков
  в конфиге меняет ответ);
  (б) `find` без пути возвращает **дубликаты** одного символа из двух серверов → `find_unique` падает
  `Found multiple N symbols matching ...`; (в) ответ «не по тому файлу» (см. #2120: `$/getFileReferences`
  подмешивается в symbol-references, и `find_referencing_symbols` на `.ts`-символе возвращает **все импорты модуля**).
- **ПРИЧИНА:** нет явного «владельца файла» (routing-таблицы) и нет дедупликации результатов по
  `(path, range, name)`; companion-серверы считаются равноправными источниками.
- **НАШ АНАЛОГ:** `lsp.*` (наш роутинг «файл → сервер»), `graph.index` (один узел — один владелец).
- **РЕГРЕССИОННЫЙ ТЕСТ:** проект с `languages: [vue, typescript]`; `find_symbol("foo")` без `relative_path` →
  ожидание: ровно 1 результат на файл (дедуп по `(path, selectionRange)`), и детерминированный выбор LS
  (тест: 10 запусков с перемешанным порядком серверов → одинаковый результат).
  Плюс: `references` на `.ts`-символе не содержит «импорт всего модуля» (проверять по `range` = имени).
- **СЕРЬЁЗНОСТЬ:** high.

### B9. `refs-crash-on-foreign-encoding`
- **ИСТОЧНИК:** issue #2017 `[open]` — https://github.com/oraios/serena/issues/2017 ;
  код: `src/solidlsp/ls.py:2570` (`file_data.contents.split("\n")` — чтение файла ссылки),
  `src/solidlsp/ls_utils.py:490-521` (`FileUtils.read_file`).
- **СИМПТОМ:** `find_referencing_symbols` падает с `UnicodeDecodeError` (не ошибка тула, а краш),
  если файл-источник ссылки в UTF-16 (бэкап, PowerShell-выгрузка), а проект в UTF-8. Весь результат теряется.
- **ПРИЧИНА:** чтение файлов ссылок не изолировано try/except на уровне «пропустить файл с warning».
- **НАШ АНАЛОГ:** `lsp.references`, `graph.index` (обход всех файлов), `edit.replace` (чтение перед правкой).
- **РЕГРЕССИОННЫЙ ТЕСТ:** проект: `main.py` (UTF-8) ссылается на `helper`; `helper.py` записан в UTF-16-LE с BOM →
  ожидание: `lsp.references` возвращает ссылки из `main.py` **и** помечает `skipped_files: ["helper.py (decode error)"]`;
  без исключения. Для `graph.index`: файл помечен `index_state: skipped`, `coverage` учитывает его как пропущенный.
- **СЕРЬЁЗНОСТЬ:** medium.

---

## C. Диагностики

### C1. `diag-empty-pull-falls-back-to-stale-publish`
- **ИСТОЧНИК:** `src/solidlsp/ls.py:1043-1048`:
  `if not ret:` → ждём published (`_wait_for_relevant_published_diagnostics`) и берём его.
  Т.е. **пустой результат pull** (файл чист!) неотличим от «pull не сработал».
- **СИМПТОМ:** после исправления ошибки `get_diagnostics_for_file` возвращает **старые** ошибки из
  кэша publishDiagnostics (сервер прислал `[]` или ещё не прислал ничего → берём cached).
  Классика: «устаревшие диагностики после правки».
- **ПРИЧИНА:** `[]` используется как sentinel «нет данных»; нет различения `None` (нет ответа) и `[]` (нет проблем).
- **НАШ АНАЛОГ:** `lsp.diagnostics` (и `lsp.check`-подобные), а также `edit.*` с диагностиками после правки.
- **РЕГРЕССИОННЫЙ ТЕСТ:** mock-LSP: pull отвечает `{"items": []}`; кэш publish содержит 2 старые ошибки →
  ожидание: `[]` (файл чист). Второй кейс: pull бросает «method not found», кэш пуст → `[]` + `source: "unavailable"`.
- **СЕРЬЁЗНОСТЬ:** high (это ровно тот баг, который ломает «правка → проверка»).

### C2. `diag-empty-publish-never-satisfies-wait`
- **ИСТОЧНИК:** `src/solidlsp/ls.py:936-976`: `_accept_published_diagnostics` = `return bool(diagnostics)`;
  `_wait_for_relevant_published_diagnostics` крутится до таймаута, если payload пустой, затем
  `if allow_cached: ... self._get_cached_published_diagnostics(uri)` → возвращает **предыдущий** (непустой) набор.
- **СИМПТОМ:** сервер корректно опубликовал «ошибок нет» (`diagnostics: []`), но Serena считает публикацию
  неудовлетворяющей, ждёт полный таймаут (2.5 s; у rust-analyzer 8 s, у Julia 30 s) и затем отдаёт
  **устаревшие** диагностики из кэша. Симптом для пользователя: «диагностики не исчезают после фикса» + задержка.
- **ПРИЧИНА:** «пусто» трактуется как «не готово» (перестраховка от преждевременных публикаций).
- **НАШ АНАЛОГ:** `lsp.diagnostics`, `graph.changes` (события «стало чисто»).
- **РЕГРЕССИОННЫЙ ТЕСТ:** mock-LSP: (1) publish `[err]` (generation 1) → тул вернул 1;
  (2) правка; (3) publish `[]` (generation 2) → ожидание: `[]` и **время ответа < 300 мс** (не ждём таймаут).
  Инвариант: пустая публикация новее запроса всегда терминальна.
- **СЕРЬЁЗНОСТЬ:** high.

### C3. `diag-diff-false-new-and-missed-cross-file`
- **ИСТОЧНИК:** `src/serena/lsp/lsp_diagnostics.py:151-205` (`DiagnosticsDiff`):
  `min_severity=2` захардкожен; `timeout=2.5` захардкожен; сравниваются только `edited_files`;
  identity = (`message`, точные start/end line/character, severity, code, source); `except:` (bare) на 163 и 179.
- **СИМПТОМ:** (а) диагностика, «съехавшая» на 1 строку из-за правки выше, считается **новой** → шум «ты сломал»;
  (б) диагностики в **других** файлах, вызванные правкой (изменил сигнатуру → 30 ошибок в вызывающих),
  не показываются вообще; (в) `Hint`/`Information` (severity 3/4) отфильтрованы навсегда;
  (г) при медленном LS (`2.5 s`) diff молча пустой — «правка не вызвала проблем».
- **ПРИЧИНА:** identity привязана к абсолютной позиции, а не к «символу+сообщению»; область — только
  отредактированные файлы; таймаут/severity не конфигурируемы; ошибки проглатываются bare `except`.
- **НАШ АНАЛОГ:** `lsp.diagnostics` (before/after), `graph.changes`, любые «post-edit checks».
- **РЕГРЕССИОННЫЙ ТЕСТ:** (1) до правки: ошибка на строке 10; правка добавляет строку выше; после — та же ошибка
  на строке 11 → ожидание: `new: []` (не «новая»). (2) правка сигнатуры → ошибка в **другом** файле →
  ожидание: попадает в отчёт как `introduced_in_other_files`. (3) LS отвечает за 5 s →
  отчёт содержит `diagnostics: incomplete (timeout)` вместо пустого.
- **СЕРЬЁЗНОСТЬ:** medium.

### C4. `diag-owner-attribution-picks-next-symbol`
- **ИСТОЧНИК:** `src/serena/symbol.py:1013-1034` (`find_diagnostic_owner_symbol` → `request_symbol_at_location`),
  `src/solidlsp/ls.py:2872-2891` — приоритеты: 0 = позиция в `selectionRange`, 1 = в `range`,
  **2/3 = точное совпадение start**, **4 = `selection_start.line == line and column <= selection_start.character`**,
  **5 = аналогично для `symbol_start`**. Т.е. при позиции «до» символа на той же строке выбирается **следующий** символ.
- **СИМПТОМ:** диагностика на отступе/в конце предыдущей строки/на `}` приписывается **следующему**
  символу (или наоборот) → группировка `severity → name_path` врёт, агент идёт «чинить» не тот символ.
- **ПРИЧИНА:** эвристика «ближайший следующий» вместо строгой проверки вложенности (range contains position).
- **НАШ АНАЛОГ:** `lsp.diagnostics` (группировка по символу), `graph.search` (привязка узла к позиции).
- **РЕГРЕССИОННЫЙ ТЕСТ:** файл: `def a():` (стр.0), тело, `def b():` (стр.3). Диагностика с
  `range.start = (3, 0)` при `column=0`, и с `(2, 20)` (конец тела `a`) → ожидание: обе привязаны к `a`
  (или `a`/`<file>`), **никогда** `b`. Инвариант: owner.range строго содержит позицию диагностики.
- **СЕРЬЁЗНОСТЬ:** medium.

### C5. `diag-severity-missing-crashes`
- **ИСТОЧНИК:** `src/solidlsp/ls.py:1032-1041`: `"severity": item["severity"]` — **обязательный** ключ
  для pull-диагностик, хотя LSP делает `severity` опциональным
  (`lsp_protocol_handler/lsp_types.py:3838-3840`: «Can be omitted»). В publish-пути значение оборачивается в
  `DiagnosticSeverity(severity)` (`ls.py:702-705`), а `DiagnosticSeverity` — `IntEnum` c 1..4
  (`lsp_types.py:512-522`), т.е. severity `0`/`5` (встречается у нестандартных серверов) → `ValueError`.
- **СИМПТОМ:** сервер, не заполняющий `severity`, валит `get_diagnostics_for_file` `KeyError`;
  сервер с severity вне 1..4 валит нормализацию `ValueError` — и это внутри **observer'а нотификаций**,
  т.е. диагностики просто перестают обновляться (ошибка уходит в лог).
- **ПРИЧИНА:** жёсткая типизация опционального поля и отсутствие fallback.
- **НАШ АНАЛОГ:** `lsp.diagnostics`, `graph.changes` (парсинг чужих payload'ов).
- **РЕГРЕССИОННЫЙ ТЕСТ:** pull: `{"items": [{"message": "x", "range": {...}}]}` (без severity) →
  ожидание: диагностика возвращена с `severity: null`/`Unknown`; publish с `severity: 0` и `severity: 7` →
  без исключения, `severity: Unknown`. Инвариант: ни один payload не роняет обработчик нотификаций
  (тест: после «плохой» публикации следующая «хорошая» всё ещё применяется).
- **СЕРЬЁЗНОСТЬ:** medium.

### C6. `diag-not-of-the-edited-file`
- **ИСТОЧНИК:** `src/solidlsp/ls.py:914-918` + `src/solidlsp/language_servers/typescript_language_server.py:620-628`
  (`_get_published_diagnostics_uri` переписывает `file:///C:/...` → `file:///c%3A...`);
  `_canonicalize_published_diagnostics_uri` (`ls.py:726-753`) работает **только** при `os.name == "nt"`.
- **СИМПТОМ:** на Windows/macOS-кейсах диагностики «не того файла» или пустые: сервер публикует под
  URI с другой нормализацией (percent-encoded drive, `file://` vs `file:/`, symlink-путь, `..`-сегменты),
  ключ не совпадает → ждём таймаут → отдаём `[]` (и C1/C2 усиливают эффект).
- **ПРИЧИНА:** сопоставление URI сделано эвристикой по строке и только для Windows; нет канонизации
  через `realpath` + case-fold на case-insensitive ФС.
- **НАШ АНАЛОГ:** `lsp.diagnostics`, `graph.index` (ключи узлов), `edit.*` (какой файл реально правим).
- **РЕГРЕССИОННЫЙ ТЕСТ:** параметризовать URI: `file:///C:/p/a.py`, `file:///c%3A/p/a.py`, `file:///c:/p/a.py`,
  `file:/p/a.py`, путь с `..` и с symlink-алиасом → ожидание: во всех вариантах диагностики находятся
  (тест на Linux через подмену `os.name`/функции канонизации). Инвариант: ключ узла/файла — канонический
  (realpath + case-fold), а не исходная строка.
- **СЕРЬЁЗНОСТЬ:** medium.

---

## D. Правки (edit)

### D1. `utf16-vs-codepoint-columns`
- **ИСТОЧНИК:** issue #2064 `[open]` — https://github.com/oraios/serena/issues/2064 ; PR #2115 `[open, не смержен]`
  («convert LSP UTF-16 positions to code points before indexing», добавляет
  `TextUtils.utf16_offset_to_codepoint_offset`); текущий код: `src/solidlsp/ls_utils.py:343-380`
  (докстринг: «columns are offsets into the Python string (code points), **not UTF-16 code units**»),
  `src/solidlsp/ls.py:1474-1515` (передаёт `column` в LSP как есть).
- **СИМПТОМ:** файл с эмодзи/не-BMP символом на строке → правки **после** него применяются со сдвигом
  на 1 code point за каждый не-BMP символ: `replace_symbol_body`/`insert_*` портят файл,
  `definition`/`references` возвращают позиции, которые при обратном применении не совпадают.
  Ошибка зависит от контента → выглядит как «случайный баг LS».
- **ПРИЧИНА:** LSP `Position.character` = UTF-16 code units; Serena индексирует Python-строку по code points;
  `general.positionEncodings` не согласуется (несколько адаптеров явно объявляют `utf-16`:
  `eclipse_jdtls.py:1212`, `rust_analyzer.py:465`, `kotlin_language_server.py:521`, `sourcekit_lsp.py:86`).
- **НАШ АНАЛОГ:** **все** `lsp.*` и `edit.*` (позиции на границе LSP↔наш буфер). У нас Rust —
  та же ловушка: `char_indices` (UTF-8 байты) vs UTF-16 code units vs code points — три разные единицы.
- **РЕГРЕССИОННЫЙ ТЕСТ:** файл: `s = "🎉🎉"; target = 1` (две эмодзи до целевого символа) →
  (1) `lsp.replace_symbol_body("target", ...)` → файл остаётся валидным, эмодзи не тронуты, тело заменено;
  (2) `lsp.references("target")` → позиция ссылки конвертируется в наш offset и обратно **идемпотентно**;
  (3) property-тест: для каждой позиции p в строке с эмодзи `to_lsp(to_internal(p)) == p`.
  Отдельный кейс: `edit.replace` с `old_string`, содержащим эмодзи (fuzzy-поиск не должен сдвигаться).
- **СЕРЬЁЗНОСТЬ:** critical.

### D2. `column-past-line-end-inserts-at-eof`
- **ИСТОЧНИК:** `src/solidlsp/ls_utils.py:371-374`:
  `change_index = min(change_index, len(text))` — «clamping a column that points beyond the end of the text».
  `get_index_from_line_col` (`ls_utils.py:291-305`) возвращает `line_start_idx + col` **без** проверки `col <= len(line)`.
- **СИМПТОМ (проверено на коде):** текст `"ab\ncd\n"`, вставка в `(line=0, col=99)` → индекс 99 → clamp до
  `len(text)=6` → текст вставляется **в конец файла**, а не в конец строки 0. То же для `delete_text_between_positions`:
  `(0,99)–(1,1)` → `ValueError: Start position ... is after end position` (бессмысленное сообщение вместо
  «column out of range»). Любой off-by-N в column (1-based от другого тула, UTF-16 vs code point) → тихая
  запись не туда.
- **ПРИЧИНА:** column не валидируется относительно длины строки; clamp сделан к концу **файла**.
- **НАШ АНАЛОГ:** `edit.replace`, `edit.patch`, `edit.hashline`, `lsp.insert_before_symbol`/`insert_after_symbol`
  (все, кто строит offset из (line, col)).
- **РЕГРЕССИОННЫЙ ТЕСТ:** (1) `insert(line=0, col=99)` в `"ab\ncd\n"` → ожидание: ошибка `column 99 out of range
  for line 0 (length 2)`; файл не изменён. (2) `insert(line=0, col=2)` (конец строки) → вставка в конец строки 0.
  (3) `insert(line=0, col=-1)` → ошибка (не «в начало файла»). (4) `insert(line=-1, col=0)` → ошибка
  (не «в начало файла», см. D3).
- **СЕРЬЁЗНОСТЬ:** high.

### D3. `negative-line-col-silently-shifted`
- **ИСТОЧНИК:** `src/solidlsp/ls_utils.py:291-305` (`get_index_from_line_col`: `while text_stepper.line < line`
  — при отрицательном `line` цикл не выполняется, возвращается `line_start_idx + col` = начало файла);
  `src/serena/code_editor.py:215-223` (`insert_at_line`), `247-256` (`_delete_line_range`).
- **СИМПТОМ (проверено):** `insert_at_line(line=-1, content)` → вставка **в начало файла** (молча);
  `delete_lines(start_line=-1, end_line=0)` → удаляется **первая** строка вместо последней.
  `read_file` при этом документирует отрицательные индексы как «count from the end»
  (`fs_api.py:187`), т.е. агент **правомерно** передаёт `-1` и получает противоположный результат.
- **ПРИЧИНА:** отрицательные индексы поддержаны в `read_file` (`TextUtils.line_slice_indices`, `ls_utils.py:407-435`),
  но **не** в edit-примитивах; семантика расходится между чтением и записью.
- **НАШ АНАЛОГ:** `edit.replace` (line-based режимы), `lsp.replace_symbol_body`, любые line-based правки;
  особенно если у нас есть `read_file` с отрицательными индексами.
- **РЕГРЕССИОННЫЙ ТЕСТ:** таблица «тот же диапазон, что вернул read_file, применяется тем же местом»:
  для `n=4` строк и каждого `i`: `read_file(i, i)` → текст строки i; `delete_lines(i, i)` → удалена строка i.
  Для `i = -1, -2`: либо одинаково трактуется (последняя/предпоследняя), либо **явная ошибка**
  `negative line indices are not supported by delete_lines` — но никогда «первая строка».
- **СЕРЬЁЗНОСТЬ:** high (тихая потеря строки).

### D4. `inverted-range-duplicates-text`
- **ИСТОЧНИК:** PR #2107 `[fixed]` — https://github.com/oraios/serena/pull/2107 ; текущий код
  `src/solidlsp/ls_utils.py:335-336` (`raise ValueError("Start position ... is after end position")`).
  До фикса: `text[del_start:del_end]` при `start>end` = пустой срез, а конкатенация **копировала** диапазон.
- **СИМПТОМ (до фикса):** `delete_lines(3, 1)` на `a\nb\nc\nd\n` → `a\nb\nc\nc\nd\n`
  (текст **удвоился**), тул ответил `Success`. Перепутанные границы — типичная ошибка LLM («строки 10-15»).
- **ПРИЧИНА:** отсутствие проверки порядка позиций в примитиве.
- **НАШ АНАЛОГ:** `edit.replace`/`edit.patch`/`edit.apply_patch` — любой диапазонный примитив; `lsp.replace_symbol_body`
  (если start/end берутся из LSP и сервер вернул инвертированный range — известный случай у Julia,
  см. комментарий `ls.py:790-800` про «normalize inverted ranges»).
- **РЕГРЕССИОННЫЙ ТЕСТ:** `delete_lines(3,1)` → ошибка `start_line 3 > end_line 1`; файл байт-в-байт не изменён.
  Плюс: LSP отдаёт `range.end < range.start` (инвертированный) → `replace_symbol_body` **отказывает**,
  а не применяет правку. Плюс инвариант: `len(new) <= len(old) + len(replacement)` (не «размножение» текста).
- **СЕРЬЁЗНОСТЬ:** high.

### D5. `replace-body-strip-kills-indentation`
- **ИСТОЧНИК:** `src/serena/code_editor.py:122-127`:
  `body = body.strip()` с комментарием «make sure the replacement adds no additional newlines … we strip it entirely».
- **СИМПТОМ:** тело, которое отдаёт `find_symbol(include_body=true)`, начинается **без** отступа первой строки
  (срез идёт от `range.start.character`, т.е. от колонки `def`), но последующие строки — с отступом.
  Если агент подставляет тело **в том виде, как оно выглядит в файле** (с отступом 4 у первой строки — обычное
  поведение LLM при копировании), `strip()` снимает отступ **только у первой строки** → в Python
  `IndentationError`, в других языках «съехавший» блок. Плюс `strip()` съедает намеренные пустые строки
  в начале/конце тела, если они были частью форматирования.
  Отдельно: контракт «body» несимметричен — то, что вернул `find_symbol`, не является тем, что ожидает
  `replace_symbol_body` при вложенных символах (см. D6).
- **ПРИЧИНА:** «нормализация» через `strip()` не учитывает контекстный отступ символа.
- **НАШ АНАЛОГ:** `lsp.replace_symbol_body`, `edit.replace` (режим замены тела), `edit.sloppy`.
- **РЕГРЕССИОННЫЙ ТЕСТ:** класс `C` с методом `m` (отступ 4): `replace_symbol_body("C/m", body)` где
  `body = "    def m(self):\n        return 2"` (с отступом) → ожидание: отступ сохранён ровно один раз
  (не 0 и не 8); результат парсится (для Python — `ast.parse` успешен). Второй кейс: body без отступа →
  отступ добавляется по контексту символа. Третий: body с 2 пустыми строками в конце → сохраняются
  (или явно задокументировано обратное).
- **СЕРЬЁЗНОСТЬ:** high.

### D6. `replace-body-duplicates-decl-keyword`
- **ИСТОЧНИК:** issues #1529 `[fixed]` (Go: `type type Resampler struct`), #1956 `[open]`
  (TS: `export const export const twice = …;;`), #1484 `[fixed]` (C#: потеря `[return: NotNullIfNotNull(...)]`);
  код-контекст: `src/serena/code_editor.py:110-127` (range берётся из `location.range`),
  `src/solidlsp/language_servers/gopls.py:29-33` (костыль: gopls отдаёт range от идентификатора, а не от ключевого слова).
- **СИМПТОМ:** body, который агент получил из `find_symbol(include_body=true)`, **не** round-trip'ится:
  если подставить в него полное объявление (как требует документация), ключевое слово/атрибут дублируется
  или теряется. Go: `type type ...` (файл не парсится, gopls теряет дерево символов → все последующие
  правки «No symbol matching»). TS: `export const export const ...;;`. C#: атрибут перед методом исчезает.
- **ПРИЧИНА:** контракт «body» не определён относительно диапазона: у разных LS range включает/не включает
  ключевое слово и атрибуты; клиент не нормализует.
- **НАШ АНАЛОГ:** `lsp.replace_symbol_body` (главный), `lsp.insert_before_symbol` (атрибуты/декораторы).
- **РЕГРЕССИОННЫЙ ТЕСТ (round-trip invariant):** для каждого поддерживаемого языка, для символов
  {top-level function, method, top-level `const`, `type/struct`, класс с атрибутом/декоратором}:
  `body = lsp.find_symbol(..., include_body=true).body` → `replace_symbol_body(body)` → **файл не изменился
  (байт-в-байт)**. Второй кейс: `replace_symbol_body(full_declaration)` (с ключевым словом) → ровно одно
  ключевое слово, файл парсится. Третий: декоратор/атрибут над символом сохраняется.
- **СЕРЬЁЗНОСТЬ:** high (самый частый «порт-баг» между LS).

### D7. `replace-body-eats-separator-line`
- **ИСТОЧНИК:** issue #1952 `[fixed]` — https://github.com/oraios/serena/issues/1952 (GDScript: исчезает
  пустая строка перед следующей функцией); CHANGELOG строка 243-249 (Godot: «parser can report a symbol's end
  column one column past the line-end convention … `replace_symbol_body` on the last function in a file silently
  consumed the separating blank line»); issue #1498 (`range` ровно на строку за EOF).
- **СИМПТОМ:** правка тела одного символа меняет **чужой** trivia: удаляется разделительная пустая строка
  перед следующим символом (или в конце файла). Diff «чистого» рефакторинга содержит лишние изменения.
- **ПРИЧИНА:** end-позиция из LS трактуется буквально; «+1 column past line end»/«+1 line» конвенции
  не нормализуются; нет проверки, что правка не выходит за `range` символа.
- **НАШ АНАЛОГ:** `lsp.replace_symbol_body`, `lsp.insert_after_symbol` (оба работают с end-позицией).
- **РЕГРЕССИОННЫЙ ТЕСТ:** два соседних символа через 1 пустую строку; `replace_symbol_body(first, ...)` →
  ожидание: пустая строка и текст второго символа не изменены (сравнение хвоста файла). Плюс: символ —
  последний в файле без trailing newline → trailing newline не добавлен/не удалён.
- **СЕРЬЁЗНОСТЬ:** medium.

### D8. `body-start-column-beyond-line`
- **ИСТОЧНИК:** `src/solidlsp/ls.py:236-285` (`SymbolBody.get_text`): `symbol_body = symbol_body[self._start_col:]`
  применяется к **склеенному** тексту строк, без проверки `start_col <= len(first_line)`.
- **СИМПТОМ (проверено):** строки `["def f():", "    return 1", ...]`, `start_col=12` (больше длины 8) →
  тело = `" return 1"` (срезано в следующую строку!). Т.е. `body`, отданный агенту, начинается не там,
  а `replace_symbol_body` с таким диапазоном удалит не то.
- **ПРИЧИНА:** отсутствие валидации start-позиции; срез по «плоскому» тексту маскирует выход за строку.
- **НАШ АНАЛОГ:** `lsp.replace_symbol_body`, `lsp.find_symbol(include_body)`, `edit.replace` (извлечение тела).
- **РЕГРЕССИОННЫЙ ТЕСТ:** символ с `range.start.character > len(line)` (некорректный LS) →
  ожидание: доменная ошибка `invalid symbol range: start column 12 exceeds line length 8`;
  `include_body` не возвращает «сдвинутое» тело. Инвариант: `body` всегда начинается с символа,
  который равен `name` (для identifier-символов).
- **СЕРЬЁЗНОСТЬ:** medium.

### D9. `stale-preview-id-hits-different-match`
- **ИСТОЧНИК:** issue #2154 `[fixed]` — https://github.com/oraios/serena/issues/2154 ; PR #2155;
  код: `src/serena/util/text_utils.py:566-576` (`make_occurrence_id` = digest(content_digest:start:end)),
  `799-845` (`select` → `ReplacementRejectedError` при несовпадении), `918-955` (`apply` пере-выводит occurrences).
- **СИМПТОМ:** dry-run выдал `occurrence_id` для 2-го вхождения `foo`; файл изменился (удалена 1-я строка);
  apply со старым id заменил **другое** вхождение, отчитавшись об успехе.
- **ПРИЧИНА:** id был привязан к индексу вхождения, а не к содержимому+смещению.
- **НАШ АНАЛОГ:** `edit.replace` / `edit.patch` (если у нас есть preview→apply с id), `edit.hashline`
  (хэши строк — тот же принцип!), `edit.apply_patch` (частичное применение).
- **РЕГРЕССИОННЫЙ ТЕСТ:** файл `a\nfoo\nfoo\nfoo\n`; preview → взять id 2-го вхождения; удалить первую строку;
  apply со старым id → ожидание: **отказ** `stale occurrence id`, файл не изменён. Плюс: apply **без** id
  на изменённом файле → применяется к текущим совпадениям, но ответ содержит `content_changed_since_preview: true`.
- **СЕРЬЁЗНОСТЬ:** high (для `edit.hashline` — прямое попадание: хэш строки ≠ хэш «места»).

### D10. `multi-file-replacement-partial-apply`
- **ИСТОЧНИК:** `src/serena/util/text_utils.py:918-955` (`MultiFileReplacement.apply`): цикл по файлам,
  каждый файл пишется **своим** `EditedFileContext`; при исключении на 3-м файле первые два уже записаны.
  Ср. issue #2109 `[fixed]` (replace_lines писал дважды) — тот же класс, но на уровне набора файлов.
- **СИМПТОМ:** «частично применённый патч»: 2 из 5 файлов изменены, тул вернул ошибку; повторный запуск
  применяет правки **повторно** к уже изменённым файлам (idempotency нарушена).
- **ПРИЧИНА:** нет двухфазного применения (validate-all → write-all) и нет отката.
- **НАШ АНАЛОГ:** `edit.apply_patch` (multi-file), `edit.patch`, `lsp.rename` (multi-file WorkspaceEdit).
- **РЕГРЕССИОННЫЙ ТЕСТ:** 3 файла, во 2-м искусственно ломаем запись (read-only / инъекция исключения) →
  ожидание: **ни один** файл не изменён (или есть `rollback: ok`), ответ содержит `applied: 0 of 3`.
  Второй кейс: повторный вызов того же патча → `0 changes (already applied)`, а не удвоение.
- **СЕРЬЁЗНОСТЬ:** medium.

### D11. `crlf-normalized-to-lf-on-edit`
- **ИСТОЧНИК:** PR #2071 `[closed, не смержен]` — https://github.com/oraios/serena/pull/2071
  («native line endings preserve the file's dominant line ending»); текущее состояние:
  `SerenaConfig.line_ending: LineEnding = LineEnding.NATIVE` (`serena_config.py:968`),
  `LineEnding.newline_str → None` (`serena_config.py:259-269`),
  `write_file_atomic(..., newline=None)` (`util/file_system.py:47`),
  чтение нормализует к LF (`FileUtils.read_file`, `ls_utils.py:490-506`). Проверено: `write(a\r\nb\r\n)` → после
  read+write с `newline=None` на Linux получаем `a\nb\nc\n`.
- **СИМПТОМ:** правка CRLF-файла (Windows-проект, MQL, .bat) на Linux/macOS **переписывает весь файл** в LF;
  при правке фрагмента — смешанные окончания (хуже, чем полная замена). Diff показывает файл целиком.
- **ПРИЧИНА:** `newline=None` = «платформенный по умолчанию», а не «как в файле»; доминирующее окончание не определяется.
- **НАШ АНАЛОГ:** `edit.replace`, `edit.patch`, `edit.apply_patch`, `edit.hashline` — **любая** запись файла.
- **РЕГРЕССИОННЫЙ ТЕСТ:** файл `a\r\nb\r\nc\r\n`; `edit.replace("b", "B")` → ожидание: результат
  `a\r\nB\r\nc\r\n` (только целевая строка изменена, окончания сохранены). Плюс: файл со **смешанными**
  окончаниями → сохраняется как было (никакой «нормализации»), либо явная ошибка. Плюс: пустой файл, файл без
  trailing newline, файл только с `\r` (старый Mac) — отдельные кейсы.
- **СЕРЬЁЗНОСТЬ:** medium (высокий шум в diff, ломает git-историю).

### D12. `edit-without-prior-read`
- **ИСТОЧНИК:** докстринги тулов: `src/serena/tools/file_tools.py:253, 278` («Requires that the same range of
  lines was previously read using the `read_file` tool to verify correctness») и
  `src/serena/repl/api/edit_api.py:207, 222` — но **никакой** проверки «было ли прочитано» в коде нет
  (grep по `read_tracking`/`_files_read` — пусто).
- **СИМПТОМ:** контракт заявлен, но не enforced: агент правит диапазон, который никогда не читал, →
  «fuzzy-совпадение не туда» на уровне строк. Это же — корень #2110 (`read_file` вернул пусто,
  агент считает, что видел содержимое).
- **ПРИЧИНА:** декларация без реализации; нет состояния «прочитанные диапазоны» на сессию.
- **НАШ АНАЛОГ:** `edit.replace`/`edit.patch` (line-based), `edit.hashline` (по своей природе требует чтения),
  `lsp.replace_symbol_body` (требует `find_symbol` с телом — тоже не enforced).
- **РЕГРЕССИОННЫЙ ТЕСТ:** `delete_lines(5, 7)` без предшествующего `read_file` → ожидание:
  ошибка `lines 5-7 were not read in this session; read them first` (или предупреждение в ответе
  с эхом удаляемого текста). Второй кейс: файл изменился после чтения (mtime/hash) →
  `file changed since it was read; re-read before editing`.
- **СЕРЬЁЗНОСТЬ:** medium (дешёвый guard, ловит целый класс «правка не туда»).

### D13. `workspace-text-edits-overlap-and-ties`
- **ИСТОЧНИК:** `src/solidlsp/ls.py:3265-3282` (`apply_text_edits_to_file`): сортировка
  `sorted(edits, key=lambda e: (start.line, start.character), reverse=True)` и затем **последовательное**
  применение (`delete` + `insert` на каждый edit) без проверки пересечений и без пересчёта последующих диапазонов.
- **СИМПТОМ (проверено):**
  (1) две правки с **одинаковым** `start` (`(0,0)-(0,0)` → `"A"` и `"B"`): сортировка стабильна, порядок
  сохраняется `[A, B]`, применение даёт `"BAxyz"` — т.е. вставки идут в обратном порядке относительно
  порядка в ответе сервера;
  (2) **вложенные** правки (`(0,0)-(0,3)` → `"OUTER"`, `(0,1)-(0,2)` → `"INNER"`): применяется `INNER`
  первым, затем `OUTER` удаляет уже сдвинутый диапазон → `"NERz"` вместо ожидаемого `"OUTER"`.
  Никакой ошибки, `num_changes == 2`, статус «успех».
- **ПРИЧИНА:** «применить по одной, с конца» корректно только для **непересекающихся** правок;
  пересечения/вложенность/совпадающие позиции не детектируются, хотя LSP их не запрещает
  (серверы шлют вложенные правки при ренейме символа внутри переименовываемого фрагмента).
- **НАШ АНАЛОГ:** `edit.apply_patch`, `lsp.rename` (multi-edit), `edit.replace` (multi-occurrence),
  `edit.hashline` (несколько правок в одном файле за вызов).
- **РЕГРЕССИОННЫЙ ТЕСТ:** (1) две вставки в одну позицию → ожидание: детерминированный результат,
  равный конкатенации в порядке ответа сервера (`"ABxyz"`), либо ошибка `overlapping edits at 0:0`.
  (2) вложенные правки → ожидание: ошибка `overlapping edits: (0,0)-(0,3) contains (0,1)-(0,2)`,
  файл не изменён. (3) 100 случайных непересекающихся правок → результат совпадает с эталонной
  одновременной реализацией (сортировка по start + сборка строки).
- **СЕРЬЁЗНОСТЬ:** high.

---

## E. Граф / индекс

### E1. `index-mtime-only-newer-detection`
- **ИСТОЧНИК:** `src/serena/ls_manager.py:340-350`:
  `elif mtime > prev_mtime: events.append((rel_path, FileChangeType.Changed))`.
- **СИМПТОМ:** файл, у которого mtime **стал меньше** (git checkout старой ветки, `rsync -t`, восстановление
  бэкапа, распаковка архива с сохранением времени, копия с другой машины), **никогда** не помечается
  изменённым → LS/индекс держит старые символы; `graph.status` рапортует «актуален».
- **ПРИЧИНА:** сравнение `>` вместо `!=` (и нет сверки по размеру/хэшу).
- **НАШ АНАЛОГ:** `graph.index` / `graph.reindex` / `graph.changes` / `graph.coverage` (инвалидация по mtime).
- **РЕГРЕССИОННЫЙ ТЕСТ:** индекс → перезаписать файл, выставив mtime **в прошлое** (`os.utime`), содержимое другое →
  ожидание: `graph.changes` показывает файл как `modified`; `graph.reindex` обновляет символы;
  `graph.coverage` не считает файл свежим. Плюс: файл изменён в ту же секунду (mtime equal) →
  детектится (сравнение по `!=` + size/hash).
- **СЕРЬЁЗНОСТЬ:** high.

### E2. `index-caches-partial-nonempty-response`
- **ИСТОЧНИК:** `src/solidlsp/ls.py:1996-2006`: кэшируется любой **непустой** ответ
  (`if response: self._raw_document_symbols_cache[...] = (fd.content_hash, response)`),
  комментарий признаёт проблему только для пустого: «An empty or None response can occur when the language server
  has not yet finished indexing … caching it would permanently serve stale data».
- **СИМПТОМ:** сервер во время индексации вернул **часть** символов (типично: 3 из 20) → это кэшируется
  под хэшем содержимого и отдаётся до следующего изменения файла. `graph.index`/`find_symbol` теряют узлы
  «навсегда» (до touch файла), при этом `graph.status` — «ok».
- **ПРИЧИНА:** «непусто» используется как признак полноты; нет версии/поколения индекса в ключе кэша.
- **НАШ АНАЛОГ:** `graph.index` / `graph.reindex` (инкрементальный reindex «теряет узлы»), `lsp.symbols`.
- **РЕГРЕССИОННЫЙ ТЕСТ:** mock-LSP: 1-й `documentSymbol` → 3 символа (indexing), 2-й → 20 (готов) →
  ожидание: после готовности индекса `graph.search`/`lsp.symbols` возвращает 20 (кэш не «залип»);
  либо первый ответ помечен `partial` и **не** кэшируется. Инвариант: кэш-ключ включает `index_generation`.
- **СЕРЬЁЗНОСТЬ:** critical (тихая потеря узлов — ровно «incremental reindex теряет узлы»).

### E3. `index-unbounded-full-scan`
- **ИСТОЧНИК:** issue #2076 `[fixed]` — https://github.com/oraios/serena/issues/2076 ;
  код: `src/solidlsp/ls.py:2200-2320` (`request_full_symbol_tree`: рекурсивный `os.scandir` +
  `request_document_symbols` на **каждый** файл), `src/serena/symbol.py:751-768`
  (без `relative_path` — по **всем** LS), докстринг: «this may be slow the first time … especially if
  `within_relative_path` is not used».
- **СИМПТОМ:** `find_symbol` без пути на большом репо не возвращается; таймаут тула ограничивает **ответ**,
  а не работу → запрос продолжает жечь CPU после того, как клиент сдался (и следующий вызов встаёт в очередь).
- **ПРИЧИНА:** нет бюджета/прогресса/отмены; обход не приоритезирован.
- **НАШ АНАЛОГ:** `graph.index` (полный), `graph.search` без scope, `lsp.find_symbol` без файла.
- **РЕГРЕССИОННЫЙ ТЕСТ:** репо с 50k файлов (или mock-LS с искусственной задержкой 1 мс/файл):
  `graph.search("foo")` без scope → ожидание: ответ в пределах бюджета с `partial: true, scanned: 5000/50000,
  hint: "narrow the scope"`; и **отмена** работы после таймаута (проверить, что фоновых задач не осталось).
- **СЕРЬЁЗНОСТЬ:** high.

### E4. `index-file-runs-wrong-language-servers`
- **ИСТОЧНИК:** issue #1965 `[fixed]` — https://github.com/oraios/serena/issues/1965 ;
  код (до фикса): `serena project index-file` итерировал все LS; сейчас `src/serena/cli.py:1005-1010`
  использует `ls_mgr.get_language_server(file)` (правильно), но `_index_project` (`cli.py:916-935`)
  идёт через `gather_source_files()` + `get_language_server(rel_path)` — т.е. зависит от корректности роутинга.
- **СИМПТОМ:** в мультиязычном проекте файл индексируется **всеми** серверами; «чужие» серверы пишут
  мусорные символы в свои кэши (`document_symbols.pkl`), которые потом отдаются как настоящие.
- **ПРИЧИНА:** индексация не маршрутизируется по владельцу файла; нет проверки «этот LS действительно
  умеет этот файл».
- **НАШ АНАЛОГ:** `graph.index` / `graph.reindex` (мультиязычный проект), `lsp.index`.
- **РЕГРЕССИОННЫЙ ТЕСТ:** проект `{a.py, b.ts, c.vue}`; после `graph.index` →
  ожидание: у каждого файла **ровно один** владелец-язык; в индексе нет узлов «python-символы из b.ts»;
  `graph.status` показывает `files_by_language: {python:1, typescript:1, vue:1}` и `skipped_by_language: 0`.
- **СЕРЬЁЗНОСТЬ:** medium.

### E5. `index-cache-version-misses-ls-version`
- **ИСТОЧНИК:** issue #1722 `[fixed]` + `src/solidlsp/ls.py:3110-3154`:
  версия кэша = `(RAW_DOCUMENT_SYMBOLS_CACHE_VERSION, ls_specific_version)` + `_document_symbols_cache_fingerprint()`;
  версия самого бинаря LS в ключ **не** входит (комментарий требует вручную инкрементить при изменениях).
- **СИМПТОМ:** после обновления LS (или смены `ls_specific_settings`, ср. #1541 «JDTLS workspace cache is not
  invalidated when `ls_specific_settings` changes») кэш отдаёт символы старого сервера: неверные range,
  отсутствующие/лишние символы, «No symbol matching» на существующем коде.
- **ПРИЧИНА:** инвалидация по «человеческому» счётчику, а не по фактическому идентификатору окружения.
- **НАШ АНАЛОГ:** `graph.index` (persist-кэш), `lsp.status`/`lsp.reload`.
- **РЕГРЕССИОННЫЙ ТЕСТ:** индекс → изменить «версию LS» (env/`ls_specific_settings`) → перезапуск →
  ожидание: кэш инвалидирован (`cache_rebuilt: true`), символы перечитаны. Плюс: смена `ls_specific_settings`
  (например `initialization_options`) инвалидирует кэш.
- **СЕРЬЁЗНОСТЬ:** medium.

### E6. `graph-duplicate-nodes-via-symlink-and-alias`
- **ИСТОЧНИК:** issue #2163 `[fixed]` — https://github.com/oraios/serena/issues/2163 (симлинки в discovery,
  ignore-правила применяются к неразрешённому пути), #2144 `[fixed]` (выход за корень), PR #2172;
  код: `src/solidlsp/ls.py:2242-2300` (`is_symlink()` → `continue` только в untrusted; иначе идём по ссылке),
  `_get_range_from_file_content`/`process_directory` — путь узла = путь обхода.
- **СИМПТОМ:** один файл попадает в индекс дважды (реальный путь + symlink-алиас, или `./a.py` vs `a.py`,
  или `pkg/../pkg/a.py`); в графе — дубликаты узлов, «ложные циклы» (A→B→A через алиас), `coverage` считает
  файл дважды (или считает непокрытым).
- **ПРИЧИНА:** идентичность узла = строка пути, а не канонический (realpath + case-fold) ключ; ignore-правила
  применяются к алиасу, а не к цели.
- **НАШ АНАЛОГ:** `graph.index` (идентичность узлов), `graph.trace` (ложные циклы), `graph.coverage`.
- **РЕГРЕССИОННЫЙ ТЕСТ:** проект с `real/a.py` и `link -> real`; `graph.index` →
  ожидание: 1 узел для `a.py` (`aliases: ["link/a.py"]`); `graph.trace` не находит цикла;
  `graph.coverage` считает файл один раз. Плюс: путь с `..` и абсолютный путь → тот же узел.
- **СЕРЬЁЗНОСТЬ:** medium.

### E7. `overload-index-unstable`
- **ИСТОЧНИК:** `src/solidlsp/ls.py:2160-2187` (`convert_symbols_with_common_parent`):
  `total_name_counts`/`name_counts` считаются **по имени** среди сиблингов; `usymbol["overload_idx"] = name_counts[name]`
  присваивается в порядке выдачи LS. `src/serena/symbol.py:163-171` (`[i]` парсится из паттерна),
  `symbol.py:349-356` (`get_name_path` печатает `name[i]`).
- **СИМПТОМ:** `name_path` вида `C/foo[1]` (а) нестабилен между рестартами LS и после правок
  (порядок символов может измениться), (б) считается по имени без учёта kind → свойство `foo` и метод `foo`
  получают `[0]`/`[1]`, и после добавления второго метода индексы **сдвигаются**; (в) агент, сохранивший
  `name_path` в памяти/задаче, после правки бьёт не в тот символ (или получает «No symbol matching»).
- **ПРИЧИНА:** суррогатный идентификатор вместо устойчивого (kind + signature + position-независимый ключ).
- **НАШ АНАЛОГ:** `lsp.find_symbol`/`rename`/`replace_symbol_body` (адресация символа), `graph.*` (id узлов).
- **РЕГРЕССИОННЫЙ ТЕСТ:** класс с двумя методами `foo` (overload) → `name_path` = `C/foo[0]`, `C/foo[1]`;
  вставить **новый** `foo` **первым** в файле → ожидание: существующие символы сохраняют свои name_path
  (или адресация по `[i]` отвергается с ошибкой `ambiguous; specify signature`).
  Плюс: свойство `foo` + метод `foo` → не должны получать общий счётчик (разные kind).
- **СЕРЬЁЗНОСТЬ:** high (ломает адресацию, а значит rename/replace/safe_delete).

---

## F. Позиции и name_path

### F1. `name-path-slash-collision`
- **ИСТОЧНИК:** issue #1797 `[fixed]` — https://github.com/oraios/serena/issues/1797 (Erlang `foo/1`),
  фикс: `src/solidlsp/language_servers/erlang_language_server.py:154-166`
  (`symbol["name"].replace("/", ARITY_SEPARATOR)`, `ARITY_SEPARATOR = "#"`);
  парсер: `src/serena/symbol.py:190-195`, `NAME_PATH_SEP = "/"` (`symbol.py:28`).
- **СИМПТОМ:** символ, в имени которого есть `/` (Erlang `foo/1`, а также имена с `/` в других языках —
  пути в именах, `a/b` в DSL), невозможно адресовать: `find_symbol("foo/1")` → `[]` (паттерн трактуется как
  вложенность), `find_referencing_symbols`/`replace_symbol_body` → `ValueError: No symbol matching`.
  При этом `find_symbol(substring_matching=true)` печатает **именно такой** `name_path` — то есть
  Serena отдаёт агенту нерабочий идентификатор.
- **ПРИЧИНА:** разделитель name_path не экранируется; нет escaping-схемы (`foo\/1`) и нет валидации
  «имя содержит разделитель».
- **НАШ АНАЛОГ:** `lsp.find_symbol` / `rename` / `replace_symbol_body` / `find_referencing_symbols`
  (любая адресация по name_path), `graph.search` (поиск по qualified name).
- **РЕГРЕССИОННЫЙ ТЕСТ:** символ с именем `a/b` (создать через mock-LSP documentSymbol) →
  (1) `find_symbol("a/b")` находит его (round-trip: `name_path` из выдачи → адресация работает);
  (2) `find_symbol("a\\/b")` (escaped) тоже; (3) `find_symbol("/a/b")` (absolute) не путает.
  Инвариант: **для любого символа** `find(name_path(s)) == [s]`.
- **СЕРЬЁЗНОСТЬ:** high.

### F2. `name-path-empty-and-double-slash`
- **ИСТОЧНИК:** `src/serena/symbol.py:190-195`: `pat.lstrip("/").rstrip("/").split("/")` →
  `"a//b"` → `["a", "", "b"]`, `"/"` → `[""]` (проверено).
- **СИМПТОМ:** `"a//b"` никогда не матчится (компонент `""` не равен ничему) — тихий `[]`;
  `"/"` (попытка «все символы») — тоже `[]`. При `substring_matching=true` компонент `""` матчится **всем**
  (`"" in name` всегда True) → неожиданные совпадения.
- **ПРИЧИНА:** нет нормализации/валидации паттерна; пустой компонент не отвергается.
- **НАШ АНАЛОГ:** `lsp.find_symbol` (парсинг path-паттернов), `graph.search` (path-фильтры).
- **РЕГРЕССИОННЫЙ ТЕСТ:** `find_symbol("a//b")` → ошибка `invalid name path: empty component`;
  `find_symbol("/")` → ошибка (не пустой результат); `find_symbol("a/", substring=True)` → трактуется как `"a"`
  (trailing slash игнорируется) и **не** матчит всё подряд.
- **СЕРЬЁЗНОСТЬ:** low.

### F3. `overload-bracket-in-real-name`
- **ИСТОЧНИК:** `src/serena/symbol.py:161-171` (`PatternComponent.from_string`): если компонент кончается на `]`
  и содержит `[`, а внутри цифры — это overload-индекс. `arr[0]` (имя массива/символа) → парсится как `arr` + idx 0.
- **СИМПТОМ:** символ с именем `arr[0]` (или `x[1]` в DSL/шаблонах) невозможно адресовать точно:
  паттерн `arr[0]` означает «символ `arr`, overload 0» → не матчит `arr[0]` (проверено: `False`).
- **ПРИЧИНА:** синтаксис индекса не экранируется и не отделён от имени.
- **НАШ АНАЛОГ:** `lsp.find_symbol` (если у нас есть overload-адресация `[i]`).
- **РЕГРЕССИОННЫЙ ТЕСТ:** символ с именем `arr[0]` → `find_symbol("arr[0]")` находит его (round-trip),
  а `find_symbol("arr[1]")` — не находит. Плюс: реальный overload `foo[0]`/`foo[1]` продолжает работать.
- **СЕРЬЁЗНОСТЬ:** low.

### F4. `eof-position-conventions`
- **ИСТОЧНИК:** issue #1498 `[fixed]`; код: `src/solidlsp/ls.py:236-262` (range, кончающийся ровно на строку
  за EOF при `col == 0`, трактуется как «до конца файла»; иначе `InvalidTextLocationError`),
  `src/solidlsp/ls_utils.py:318-333` (delete: то же для end-позиции), `ls_utils.py:358-370` (insert: «строка за
  последней» = append с добавлением `\n`).
- **СИМПТОМ (до фикса):** `IndexError` при извлечении тела последнего символа; после фикса — корректно,
  но **только** для `(n_lines, 0)`. Любая другая «позиция за EOF» теперь ошибка (это правильно), однако
  LS, отдающий `(n_lines, 0)` для **не** последнего символа, получит «до конца файла» → тело символа
  растянется до EOF (тихо).
- **ПРИЧИНА:** эвристика «одна строка за EOF = до конца файла» без проверки, что символ действительно последний.
- **НАШ АНАЛОГ:** `lsp.replace_symbol_body`, `edit.replace`, любой парсер LSP-range (наш Rust-буфер).
- **РЕГРЕССИОННЫЙ ТЕСТ:** (1) последний символ, range.end = `(n_lines, 0)` → тело = до конца файла, без `IndexError`.
  (2) **средний** символ с range.end = `(n_lines, 0)` (некорректный LS) → ошибка, а не «до EOF».
  (3) пустой файл (`""`) → 0 символов, правки дают внятную ошибку, не panic. (4) файл из одной строки без `\n`.
- **СЕРЬЁЗНОСТЬ:** medium.

### F5. `zero-vs-one-based-line-boundary`
- **ИСТОЧНИК:** `src/serena/repl/api/fs_api.py:181-196` (`read_file` документирует **0-based** и
  отрицательные индексы), `src/serena/tools/file_tools.py:257-259` (`delete_lines` — **0-based**),
  `src/serena/util/text_utils.py:52-56` (`TextLine.format_line` печатает `line_number` **как есть**, т.е. 0-based),
  `src/solidlsp/ls_utils.py:407-435` (`line_slice_indices` — отрицательные индексы + clamping).
- **СИМПТОМ:** смешение 0-based (инструменты) и «человеческих» 1-based номеров (вывод поиска/диагностик,
  сообщения компилятора) приводит к правкам на строку мимо; `read_file` при `start_line` за EOF возвращает
  **пустую строку без ошибки** (issue #2110 `[fixed, частично]`: `end_line=-1` теперь трактуется как «до конца»,
  но `start_line=5` на 3-строчном файле по-прежнему даёт `[]` — проверено по `line_slice_indices`).
- **ПРИЧИНА:** нет единого контракта «все номера 0-based, вне диапазона — ошибка»; часть API «clamp'ит», часть — нет.
- **НАШ АНАЛОГ:** `edit.replace`/`edit.patch` (line-based), `lsp.diagnostics` (номера строк в отчёте),
  `graph.*` (позиции узлов), `jev` (если он отдаёт решения с номерами строк).
- **РЕГРЕССИОННЫЙ ТЕСТ:** для файла из 3 строк: `read_file(start_line=5)` → ошибка `start_line 5 beyond EOF (3 lines)`,
  а не `""`. Плюс: `read_file(i, i)` и `delete_lines(i, i)` для всех i согласованы (см. D3).
  Плюс: в **любом** выводе (диагностики, поиск) номера строк помечены как 0-based или сконвертированы в 1-based
  последовательно — тест на строку `"line 3"` при `range.start.line == 2`.
- **СЕРЬЁЗНОСТЬ:** medium.

---

## Топ-11 тестов, которые надо написать первыми

Порядок = (вероятность у нас) × (цена ошибки) × (дешевизна теста). Все — на mock-LSP/юнит-уровне, без реальных серверов.

| # | Тест | Ловит | Почему первым |
|---|------|-------|----------------|
| 1 | **`safe_delete` при пустых references + текстовом совпадении в проекте → отказ** (A7) | тихое удаление живого кода | необратимая операция + самая частая причина «пустых» references (B2/B3) |
| 2 | **UTF-16 ↔ code point ↔ UTF-8 идемпотентность позиций** (D1) | сдвиг всех правок после эмодзи/не-BMP | затрагивает **все** lsp.* и edit.*; апстрим до сих пор не починил (#2064, PR #2115 open) |
| 3 | **`column > len(line)` и `line < 0` → ошибка, а не «в конец файла/в начало»** (D2, D3) | тихая запись не туда | один guard закрывает 4 разных класса (off-by-one, 1-based, UTF-16, LLM-ошибки) |
| 4 | **`body` round-trip: `find_symbol(include_body)` → `replace_symbol_body` → файл байт-в-байт** (D6, D7) | дублирование `export const`/`type`, потеря атрибутов, съеденный разделитель | самый частый «порт-баг» между LS; 3 issue в апстриме |
| 5 | **`rename`: partial WorkspaceEdit → отказ/флаг `partial` + список непокрытых файлов** (A1, A6) | «успешный» ренейм, оставивший ссылки | ложный успех = агент идёт дальше по сломанному коду |
| 6 | **`diagnostics`: пустой pull/publish не должен отдавать кэш; «стало чисто» — терминально и быстро** (C1, C2) | устаревшие диагностики после правки | ломает весь цикл «правка → проверка»; два независимых бага в одном месте |
| 7 | **`graph.index`: частичный ответ во время индексации не кэшируется; mtime «в прошлое» детектится** (E1, E2) | тихая потеря узлов, «индекс актуален», хотя нет | именно то, что просили: «инкрементальный reindex теряет узлы» |
| 8 | **`name_path` round-trip для имён с `/`, `[0]`, пустых компонент** (F1, F2, F3) | неадресуемые символы | инвариант `find(name_path(s)) == [s]` — 10 строк кода, ловит 3 issue |
| 9 | **`rename_file` шлёт willRenameFiles/didRenameFiles и обновляет импорты (или предупреждает)** (A5) | файл переименован, импорты/индекс LS — нет | у нас есть отдельный `lsp.rename_file`, аналог у Serena отсутствует → мы в зоне риска |
| 10 | **Multi-file правка атомарна: падение на 2-м файле → 0 изменённых файлов; повторный apply идемпотентен** (D10, D9) | частично применённый патч | `edit.apply_patch` — самый «массовый» инструмент, откат потом руками |
| 11 | **Несколько TextEdit в одном файле: пересечения/вложенность/одинаковые start → детерминированно или ошибка** (D13) | тихая порча при multi-edit | ловится одним property-тестом против эталонной реализации; затрагивает `edit.apply_patch`, `lsp.rename`, `edit.hashline` |

**Бонус (дёшево, ложится рядом):** `overload_idx` стабильность (E7) и «`refs` не теряет вызывающих с тем же именем» (B1) —
оба пишутся на scripted-LSP за 20 строк и защищают адресацию/полноту ссылок.

---

## Приложение: карта «issue → наш тул»

| Serena issue | Наш тул(ы) |
|---|---|
| #1744 partial rename | `lsp.rename` |
| #1956/#1529/#1484 replace_symbol_body corrupts decl | `lsp.replace_symbol_body` |
| #1952/#1498 separator/EOF | `lsp.replace_symbol_body`, `lsp.insert_after_symbol` |
| #1697 whitespace heuristic | `lsp.safe_delete_symbol`, `lsp.insert_before/after_symbol` |
| #2109/#2107 replace_lines double write / inverted range | `edit.replace`, `edit.patch` |
| #2154 stale occurrence id | `edit.hashline`, `edit.patch`, `edit.apply_patch` |
| #2071 CRLF | все `edit.*` |
| #2064 UTF-16 | все `lsp.*` + `edit.*` |
| #2133 dropped same-name callers | `lsp.references` |
| #1937/#1858/#1814/#1681 partial refs | `lsp.references`, `graph.trace` |
| #2019 empty implementations | `lsp.implementation` |
| #2099 declaration in jar | `lsp.declaration`, `lsp.definition` |
| #2017 encoding crash | `lsp.references`, `graph.index` |
| #2120 companion-server refs merged | `lsp.references` (мультиязычные файлы) |
| #1613/#1770 diagnostics fallback timeout | `lsp.diagnostics` |
| #2076 unbounded find_symbol | `graph.search`, `lsp.find_symbol` |
| #1965 index-file wrong LS | `graph.index` |
| #1722/#1541 cache invalidation | `graph.index`, `lsp.reload` |
| #2163/#2144 symlink/root escape | `graph.index`, `edit.*` (path confinement) |
| #1797 name_path slash | `lsp.find_symbol` и все адресующие тулы |
| #2110/#2107 line range semantics | `edit.*`, `lsp.diagnostics` |
| #2003/#2004 timeout vs restart | `lsp.*` (наш restart/reload путь) |

---

## Приложение 2: что проверено эмпирически (не только прочитано)

Воспроизведено локально на копиях функций Serena (код скопирован 1:1 из клона; чужой репозиторий не изменялся):

| Проверка | Результат |
|---|---|
| `NamePathMatcher("prn_to_sv_id/1")` против компонента `["prn_to_sv_id/1"]` | `False` (F1 подтверждён: точный `name_path` из выдачи не матчится) |
| `NamePathMatcher("a//b")` | `False` всегда; `"/"` → `[""]` (F2) |
| `NamePathMatcher("arr[0]")` против символа с именем `arr[0]` | `False` (F3) |
| `TextUtils.get_index_from_line_col("ab\ncd\n", 0, 99)` | `99` → затем `min(..., len)=6` → вставка **в конец файла** (D2) |
| `get_index_from_line_col(t, -1, 0)` | `0` (начало файла) — для `insert_at_line(-1)`/`delete_lines(-1, 0)` (D3) |
| `delete_text_between_positions("a\nb\nc\nd\n", 3, 0, 2, 0)` | `ValueError: Start position ... is after end position` (D4 — фикс на месте, регресс-тест всё равно нужен) |
| `SymbolBody.get_text(start_col=12)` на строке длиной 8 | `" return 1"` — срез «перетекает» в следующую строку (D8) |
| `delete_lines(3,3)` на 3-строчном файле без trailing `\n` | `deleted=''`, файл не изменён, ответ `Success` (F5) |
| `line_slice_indices(5, None, 3)` | `(5, 3)` → `[]` без ошибки (F5 / #2110) |
| read(`a\r\nb\r\n`) + write(`newline=None`) на Linux | `a\nb\n…` — CRLF → LF (D11) |
| `DiagnosticSeverity(0)`, `DiagnosticSeverity(5)` | `ValueError` (C5) |
| `apply_text_edits_to_file`: две вставки в `(0,0)`, затем вложенные правки | `"BAxyz"` и `"NERz"` — тихая порча (D13) |
| `write_file_atomic` (модуль) | не запускался: в окружении нет `pathspec`/`sensai`; логика проверена чтением (`newline=None` → платформенное окончание) |
| Полный прогон тестов Serena | не запускался намеренно (read-only, чужие зависимости LS не устанавливались) |

Проверено чтением кода (grep по всему `src/`):
- `will_rename_files` / `did_rename_files` — **только определения**, ни одного вызова (A5);
- трекинг «файл был прочитан перед правкой» — **отсутствует** (D12), при том что докстринги его требуют;
- `apply_text_edits_to_file` (`ls.py:3265-3282`) — **не** пересчитывает сдвиги после каждой правки
  (сортировка «latest first» компенсирует это только для непересекающихся правок одного документа;
  правки, отсортированные по `start` в обратном порядке, но с **одинаковым** start, применяются в
  произвольном порядке → для двух вставок в одну позицию результат зависит от стабильности сортировки).
