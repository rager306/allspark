# oh-my-pi: глубокий инвентарь фишек для форка prime-agent

Источник: `/tmp/omp-src` (oh-my-pi / `omp`, workspace version 18.10.4, read-only).
Проверка: только чтение файлов, сборки/тесты не запускались.
Контекст-ограничение prime: `Cargo.toml:49` — `unsafe_code = "forbid"` (workspace lints).

Легенда «unsafe»: число `unsafe {`-блоков + `unsafe fn` + `unsafe impl` в крейте (grep по `crates/<crate>/**/*.rs`).

---

## 1. Таблица Rust-крейтов

| Крейт | LOC | Назначение | Ключевые типы/функции | unsafe | Применимость в prime |
|---|---:|---|---|---|---|
| `pi-edit` | 30 286 | Движок правок: парсинг/матчинг/применение 5 режимов + streaming preview + snapshots/clipboard/no-op guard | `EditMode`, `ModeEngine`, `Session`, `EditStore`, `StagedFile`, `EditWriter` | **0** | ★★★★★ прямой перенос как `pa-edit` |
| `pi-ast` | 5 293 | tree-sitter/ast-grep: реестр языков, block-анализ, AST-match/rewrite, structural summary, bounded parse cache | `block_range_at`, `enclosing_block_boundaries`, `node_chain_at`, `summarize_code`, `compile_search_patterns`, `collect_matches`, `rewrite_source` | **0** | ★★★★★ для `pa-lsp`/`pa-graph`/edit-гибрида |
| `pi-diff` | 1 020 | jsdiff-совместимый Myers O(ND) + line/word/structured-patch (без FFI, без deps) | `myers_diff`, `intern`, `line_runs_str`, `structured_patch_hunks_*` | **0** | ★★★★★ замена `pa-core/src/tools/jsdiff.rs` |
| `pi-walker` | 7 757 | Параллельный обход ФС: ignore-rules, globset, ranked collection, LRU scan-cache, heartbeat/cancel | `WalkRequest`, `WalkFilter`, `WalkOptions`, `execute_candidates`, cache API | 25 (libc/tests) | ★★★★☆ обход репо для graph/search |
| `pi-vfs` | 6 408 | Инъектируемая async ФС + sync-фасад, native fast-path, URL-preserving лексика | `Fs`, `BlockingFs`, `FileSystem` trait, `File`, `Metadata` | 38 (unix/windows OsStr) | ★★★★☆ URL/виртуальные пути, sandbox |
| `pi-shell` | 41 715 | Встроенный persistent shell (brush) + минимизатор вывода + process plumbing | `Shell`, `MinimizerConfig`, minimizer filters | 41 | ★★☆☆☆ слишком тяжёл; взять минимизатор |
| `pi-builtins` | 111 922 | Форк POSIX/bash builtins + in-process coreutils (cat/grep/sed/find/jq/ls/…) | `default_builtins`, `Host`/`Utility` trait | 126 | ☆☆☆☆☆ не нужно prime |
| `pi-natives` | 86 448 | N-API cdylib: JS-фасад над всеми крейтами + текст/highlight/grep/pty/voice/desktop/… | `EditSession`, `TextPredictor`, `ast_grep`, `ast_edit`, `grep`, `CancelToken` | 405+60 fn+7 impl | ☆☆ (napi) — prime не N-API |
| `pi-predict` | 8 284 | Word-completion: ngram + SmolLM2-135M (CPU/Metal) + macOS NSSpellChecker | `Predictor`, `Method`, `open`, `Query`/`Suggestion` | 23 (metal/appkit) | ★☆☆☆☆ только как идея |
| `pi-iso` | 4 845 | Изоляция: APFS clone / btrfs / ZFS / reflink / overlayfs / ProjFS / git-worktree | `IsolationBackend`, `BackendKind`, `Diff`, `FileChange` | 38+16 fn | ★★☆☆☆ sandbox/worktree |
| `pi-vcs` | 13 192 | In-process git (gitoxide `gix`) + Jujutsu (`jj-lib`); CLI только для network | `Repo`, `GitRepo`, `JjWorkspace`, `Feature` | 1 | ★★★☆☆ git-операции без spawn |
| `pi-voice` | 3 708 | Микрофон/playback + Opus/WebRTC live-peer | `audio`, `live::LivePeerCore` | 94+43 fn | ☆☆☆☆☆ |
| `vendor/*` | 69 782 | brush-core/parser, cfg_aliases, napi, tree-sitter-go (патчи) | — | 0 | n/a |

Итого первый-партийный Rust ≈ **320 878 LOC**. Из них «чистые» (0 unsafe): `pi-edit`, `pi-ast`, `pi-diff` — ровно те три, что нужны prime.

---

## 2. pi-edit: режимы правок (детально)

Архитектура (`crates/pi-edit/src/lib.rs:1-14`):
`raw arg deltas → stream_json::ArgStream → ArgSnapshot → ModeEngine::preview (streaming) | ModeEngine::stage → StagedFile[] → EditWriter (host)`.
Engines — чистые функции над `FileSource` (read-through cache) и `EditStore`. Строки ошибок байт-в-байт совпадают с TS-реализацией (модели на них обучены, `error.rs:1-5`).

Контракт движка (`engine.rs:199-226`): `mode()`, `preview(args,streaming,files,store)`, `stage(args,files,store)->Result<Vec<StagedFile>,EditError>`, `inspect(args)->Inspection`. `EditMode` (`engine.rs:15`): `Replace|Patch|ApplyPatch|Hashline|Sloppy`. `FileOp` (`engine.rs:51`): `Create|Update|Delete|Noop`.

| Режим | Механика | Guards | Token-цена промпта | Типовые ошибки |
|---|---|---|---|---|
| **replace** (`modes/replace.rs`, 279 LOC) | `old_string`→`new_string` (+ `replace_all`, batch `edits[]` для Cursor). Fuzzy whitespace через `fuzzy::replace_text` | непустой `old_string`; уникальность (иначе «Found N occurrences»); no-op → ошибка | ~289 tok (`prompts/replace.md`, 1159 chars) | not found; duplicate; no-change |
| **patch** (`modes/patch.rs`, 1954 LOC) | JSON `edits[]` `{op,rename?,diff?}`; hunks `@@`/`@@ $ANCHOR`; `first_match` для Codex-совместимости | anchor/context уникальность; overlap; variant ladder (`TrimCommon/DedupeShared/CollapseRepeated/SingleLine`) | ~687 tok (`prompts/patch.md`) | «Found multiple matches»; «No match found» |
| **apply_patch** (`modes/apply_patch.rs`, 541 LOC) | Codex `*** Begin Patch`/`*** Add File`/`*** Update File`/`*** Move to`/`*** Delete File`; разворачивается в patch-entries | envelope обязателен; пути относительные; атомарность | ~528 tok (`prompts/apply_patch.md`) | пустой патч; битый envelope; no supported op |
| **hashline** (16 файлов, ~7 000 LOC) | `[PATH#TAG]` секции: `PUT N.=M:`, `PUT N*:`, `PUT <N:`/`>N:`, `CUT`, регистры `@name`, `REM`, `MV DEST` | 4-hex tag обязателен; **enforce_seen_lines**; stale-tag recovery; no-op limit=3; block-resolver | ~488 tok (`prompts/hashline.md`) / ~422 tok (compact, `edit/hashline-compact.md`) | missing tag; unseen lines; stale hash; block unresolved; boundary ambiguity |
| **sloppy** (`modes/sloppy/{parse,apply}.rs`, 4865 LOC) | `*** Edit File:` + `*** Find`/`*** Replace`/`*** Insert Before|After`; `…`-captures; tolerant matching ladder | Find уникален (иначе ` all`); raw-body без diff-префиксов; атомарность секций | ~555 tok (`prompts/sloppy.md`) | missing target; no match; empty register paste |

### 2.1 hashline — механика и guards (главный кандидат)

- **Snapshot tag**: 4-uppercase-hex = `xxh32` по тексту с обрезанными trailing space/tab/CR (`store.rs:79-96`; тесты `store.rs:~430` — `"a \n b\t\r\nc"→"80BA"`).
- **Store** (`store.rs`): `DEFAULT_MAX_PATHS=256`, `DEFAULT_MAX_VERSIONS_PER_PATH=4`, `DEFAULT_MAX_TOTAL_BYTES=64 MiB`, `MAX_SNAPSHOT_FILE_BYTES=4 MiB`, `NOOP_HARD_LIMIT=3` (строки 15-25). LRU-eviction по touched-clock; `seen_lines` — `Arc<BTreeSet<u32>>` с provenance.
- **Seen-lines guard** (`modes/hashline/patcher.rs:349` `assert_seen_lines`): если anchor-строка не входила в displayed set → reveal до `SEEN_LINE_REVEAL_CAP=40` строк × `512` колонок (`patcher.rs:33-34`), записать их в seen-set и вернуть ошибку с фактическим содержимым. Т.е. следующий retry с тем же header проходит без re-read (`messages.rs:810` `unseen_lines_message`).
- **Shift rescue** (`patcher.rs:127-346`): после сдвигающих правок stale-номер можно «спасти», только если body несёт текущий контент обоих концов диапазона (`body_targets`), evidence ≥ 8 символов и уникален (`SHIFT_RESCUE_EVIDENCE_MIN_CHARS=8`). Pure inserts/cuts не спасаются.
- **Stale-tag recovery** (`modes/hashline/recovery.rs`): применяется только если цепочка snapshot'ов доказывает уникальный безопасный результат; иначе mismatch с контекстом.
- **Mismatch diagnostics** (`modes/hashline/mismatch.rs:80-118`): различает «tag распознан, файл изменился» vs «tag не из этой сессии» + перечисляет пути, где эта сессия уже выдавала tag; прикладывает `±MISMATCH_CONTEXT=2` строки контекста.
- **No-op guard** (`patcher.rs:59-84`, `store.rs:422`): byte-identical правка → soft-diagnostic; 3-я подряд → hard stop «Cease re-issuing this payload».
- **Block-op** (`modes/hashline/block.rs` + `pi-ast::block`): `PUT N*:` резолвит синтаксический блок через tree-sitter; `INSERT_AFTER`-вариант при нерезолве понижается до plain `PUT >N:` с warning; `RESOLUTION_CACHE` (512) по `(xxh64(text),len,line,path)`.
- **Amplification limit**: `MAX_EXPANDED_RANGE_LINES=100_000` (`parser.rs:28,668`).
- **Boundary repair** (`modes/hashline/apply.rs`): синтаксический probe tree-sitter — «repair boundary echo»/«retain syntax-essential row» с warning; `edit_broke_parse_warning` — advisory, не откат.
- **Clipboard** (`modes/hashline/clipboard.rs`, `store.rs:41-70`): named registers persist across calls, anonymous batch-local; interleaved same-path sections → reject (`CLIPBOARD_INTERLEAVED_SECTIONS`).
- **Path recovery** (`patcher.rs:recover_target`): если authored path не существует, но tag+filename совпадают с ровно одним stored snapshot → rebind.
- **Грамматика constrained decoding** (`grammars/hashline.lark`) — lark-грамматика для llguidance.

### 2.2 Token-цена (замер по файлам)

| Артефакт | chars | ≈tok |
|---|---:|---:|
| `prompts/replace.md` | 1 159 | ~289 |
| `prompts/patch.md` | 2 751 | ~687 |
| `prompts/apply_patch.md` | 2 112 | ~528 |
| `prompts/hashline.md` | 1 955 | ~488 |
| `edit/hashline-compact.md` | 1 688 | ~422 |
| `prompts/sloppy.md` | 2 220 | ~555 |

Per-edit tag overhead: `[PATH#TAG]` ≈ 1-3 tok. `MAX_EDIT_SNAPSHOT_TEXT_CHARS=32 768` (UTF-16) — выше этого old/new text в результате прунятся (`session.rs:117`, `prune_snapshots`).

### 2.3 Selection / fallback

`utils/edit-mode.ts:resolveEditMode`: 1) per-model variant, 2) `PI_EDIT_VARIANT`, 3) `edit.mode` (default `hashline`), 4) **fallback на `replace`** для классов `kimi|mimo|minimax|deepseek|stepfun|codex-spark|glm-flash-5.3` (модели плохо держат hashline). Настройки: `edit/settings.ts` (`fuzzyMatch`, `fuzzyThreshold=0.95`, `enforceSeenLines=true`, `blockAutoGenerated=true`, `autoRepair.enabled=false`).

---

## 3. LSP-подсистема (`packages/coding-agent/src/lsp/`, ~9 850 LOC TS)

### 3.1 client.ts (63.9 KB) — жизненный цикл и JSON-RPC
- Кэш клиентов по identity `(spawn cmd, cwd, args, initOptions, settings, languageId)` (`client.ts:1003 getOrCreateClient`).
- **Shared mux** (default `lsp.shared=true`): сначала broker-управляемый per-project mux (Unix socket / named pipe), иначе приватный `ptree.spawn()`. `muxServerKey` = `sha256(command,args,cwd,sorted env)` (`mux/protocol.ts`) — env сортируется и хэшируется, чтобы не течь credentials.
- **Negative cache** init-ошибок `INIT_FAILURE_BACKOFF_MS=3min` (`client.ts:49`) → сломанный сервер не ре-спавнится на каждый вызов.
- **pendingDiskWrites** (refcount, `client.ts:46,1250`): пока write in-flight, overlay опережает диск → `reconcileFileFromDisk` не откатывает сервер к старому содержимому.
- **Idle reaper** (`isIdleClient`, `client.ts:94`): клиент с `pendingRequests>0` никогда не idle; `lastActivity` штампуется при settle (фикс #8390).
- **sendRequest** (`client.ts:1757`): abort → `$/cancelRequest`; timeout; settle refresh `lastActivity`; write через bounded `queueWriteMessage`.
- `applyWorkspaceEditWithLsp` (`:658`) для server-initiated `workspace/applyEdit`.

### 3.2 tool.ts (53.1 KB) — единый `LspTool`
Actions (`types.ts:9`): `diagnostics|definition|type_definition|implementation|references|hover|symbols|rename|rename_file|code_actions|status|reload|capabilities|request`.
Ветки: `:221 status`, `:269 diagnostics`, `:468 rename_file`, `:821 capabilities`, `:873 request`, `:985 symbols(ws)`, `:1090 reload(ws)`, `:1205 switch` (single-file).
Read-only set `LSP_READONLY_ACTIONS` (`servers.ts:28`); всё остальное — write-tier approval.
Дизайн-фишки:
- `symbol` + `#N` occurrence-selector, identifier-boundary матчинг (`utils.ts:resolveSymbolColumn`).
- `file:"*"` = workspace-режим; `rename_file` рассылает `willRenameFiles`/`didRenameFiles` во все non-custom серверы с подходящими fileTypes.
- Project-aware серверы (Roslyn/tsserver) требуют явный `symbol` при `line` (не откатываются на first-non-ws column).
- `references`: retry ×2 (250 ms) пока результат пуст/только declaration (`diagnostics.ts:147`).
- `code_actions`: list по умолчанию, `apply:true` + `query` = client-side selector (index или substring).

### 3.3 diagnostics.ts (21.6 KB) — freshness и бюджеты
Константы (`diagnostics.ts:34-74`):
`SINGLE_DIAGNOSTICS_WAIT_TIMEOUT_MS=3 000`, `PROJECT_DIAGNOSTICS_WAIT_TIMEOUT_MS=10 000`, `BATCH_DIAGNOSTICS_WAIT_TIMEOUT_MS=400`, `INLINE_DIAGNOSTICS_WAIT_TIMEOUT_MS=500`, `DEFERRED_DIAGNOSTICS_WAIT_TIMEOUT_MS=12 000`, `DIAGNOSTICS_PIPELINE_GRACE_MS=10 000`, `MAX_GLOB_DIAGNOSTIC_TARGETS=20`, `WORKSPACE_SYMBOL_LIMIT=200`, `REFERENCE_CONTEXT_LIMIT=50`, `DIAGNOSTIC_MESSAGE_LIMIT=50`.
Freshness: push-diagnostics принимаются только по совпадению document-version или после settle; иначе `textDocument/diagnostic` (pull). `filterOrphanProjectDiagnostics` отсеивает ложные TS-коды (1375/1378/2307/2580/2591/2792/2867) вне root-marker ancestor.

### 3.4 deferred-diagnostics.ts + diagnostics-ledger.ts
- `DiagnosticsLedger.reduce` (`diagnostics-ledger.ts`): дедуп диагностик по identity = сообщение без `path:line:col `-префикса; возвращает только новые; пустой current → удаляет запись.
- `DeferredDiagnostics` (`deferred-diagnostics.ts`): медленный сервер не блокирует edit — in-flight fetch передаётся в late-injection channel; `isStale()` по mutation-version; `AbortController` на файл.

### 3.5 writethrough.ts (23 KB) — пост-обработка write
`createLspWritethrough(cwd,{enableFormat,enableDiagnostics})` (`:630`). Пайплайн `runLspWritethrough` (`:358`):
1) capture `minVersions` baseline **до** sync; 2) `syncFileContent`; 3) format in-memory; 4) sync formatted; 5) write to disk; 6) `notifyFileSaved`.
- **Деферрал** (`:308 fetchDiagnosticsWithDeferral`): inline ждёт `INLINE=500 ms`; если не успело — fetch уходит в background (inner budget 12 s), результат прилетит позже через `onDeferredDiagnostics`; без deferred-канала блокирует на стандартный бюджет.
- **tsserver-специфика**: `reloadTypeScriptProjectsAfterCreate` (`:45`) — для `typescript-language-server` после создания файла зовёт `typescript.tsserverRequest reloadProjects`, иначе cached missing-module резолюция «залипает» (особенно macOS FSEvents).
- **Batch**: `getLspBatchRequest` (`lsp/batch.ts`) — edit/write в одном model-response шарят один batch-id; flush только на последнем write.
- **Write drift**: если диск после write отличается (IDE format-on-save) → warning + re-key snapshot (`messages.rs:712 write_drift_warning`).

### 3.6 servers.ts / config.ts / defaults.json
- `defaults.json` — **57 серверов** с `command/args/fileTypes/rootMarkers/initOptions/settings/capabilities`.
- Авто-детект: cwd содержит rootMarker **и** бинарь резолвится (сначала project-local bins: `node_modules/.bin`, venv, binstubs, `bin/`, затем `$PATH`). Root-markers только cwd, не ancestors.
- Конфиг-мердж (lowest→highest): home → plugin roots → user config dirs (`~/.claude|codex|gemini`) → cwd dirs (`.omp|.claude|...`) → cwd root (`lsp.*`). Shallow merge per server (объектные поля заменяются целиком).
- `lsp.lazy=true` (default): серверы стартуют on-first-use; `discoverStartupLspServers`/`warmupLspServers` (5 s init timeout).
- `reload`: для rust-analyzer сначала `rust-analyzer/reloadWorkspace`, иначе `workspace/didChangeConfiguration` + refresh open files.

### 3.7 workspace-diagnostics.ts / edits.ts / clients/
- Workspace `diagnostics file:"*"`: детект Rust→TS→Go→Python, ≤2 чекера параллельно (`MAX_CONCURRENT_CHECKERS=2`); `cargo check --message-format=short`, `npx tsc --noEmit`, `pyright`, `go build` (go.work приоритетнее go.mod).
- `edits.ts:applyTextEditsToString`: правки применяются bottom-to-top; `sortAndValidateTextEdits` отвергает overlap; tiebreak по индексу для same-position inserts.
- `clients/` — custom linter adapters: `BiomeClient`, `SwiftLintClient`, `LspLinterClient` (диагностика/формат вне LSP-протокола).

---

## 4. Agent loop + compaction + snapcompact

### 4.1 Agent loop (`packages/agent/src/agent-loop.ts`, 4 030 LOC)
- Append-only context (`append-only-context.ts`): `StablePrefix` (system+tools заморожены) + `AppendOnlyLog` (сообщения только растут) → максимальный hit provider prefix-cache. Инвалидация только явная (MCP reconnect и т.п.).
- Speculative execution (`speculative-execution.ts`, 961 LOC): кандидаты tool-calls запускаются заранее, коммитятся по dependency-графу; `admittedArgumentsJson` фиксирует admission-time args.
- **Leak mitigation** (`harmony-leak.ts`, `dsml-leak.ts`): детект и recovery tool-call markup, просочившегося в текст (harmony `<|call|>`, DSML); nudge-промпт с cap `MAX_DSML_LEAK_NUDGES`.
- `tool-call-loop-guard.ts`: канонизация args (сортировка ключей, drop `__intent`), детект повторяющегося tool-call между ходами → steer.
- `output-budget.ts`, `tokenizer.ts`, `sent-tool-definitions.ts` (кэш отправляемых определений).

### 4.2 Compaction (`packages/agent/src/compaction/`, ~10 000 LOC)
Триггеры (`docs/compaction.md`): manual `/compact`; overflow recovery; incomplete-output (`stopReason=length`); threshold (post-turn + mid-turn); idle.
Механизмы:
- **pruning.ts** (`pruneToolOutputs`): protect newest `40 000` tool-output tok; min savings `20 000`; никогда не прунит `<50` tok; placeholder `[Output truncated - N tokens]`. **Cache-aware timing**: учитывает provider prompt-cache lookback (catalog `prompt-cache-lookback`; Claude=20 позиций) — не ломает prefix-cache.
- **shake.ts** (480 LOC): механическое замещение tool-result/fenced/XML блоков `artifact://`-ссылками. Presets: `DEFAULT` (protect 16 000, minSavings 4 000), `AGGRESSIVE` (protect 4 000), `RESCUE` (protect 0). Fence-блок ≥ `400` tok.
- **tool-protection.ts**: matcher'ы защиты (`skill://` reads, `artifact://` recovery reads) — их нельзя элидировать (иначе бесконечный цикл).
- **Useless-result elision**: tool может пометить результат `useless` (пустой поиск, wait-cap) → `[Uneventful result elided]`, но только cache-aware и не при `isError`.
- **Native replay** + **handoff** method + `compaction-v2-streaming.ts` (provider-native streaming).
- **Experimental notes-backed context**: `context_notes`/`new_context` tools, notebook ≤16 384 bytes, `history://current/full` для recovery.

### 4.3 snapcompact (`packages/snapcompact/`, 4 493 LOC TS + `pi-natives/src/snapcompact.rs`)
Ключевая необычная фишка: **bitmap-frame context compression** — discarded history рендерится в плотные PNG-кадры пиксельного шрифта, которые vision-модель читает обратно. Без LLM/API/сети, детерминированно.
- Shapes provider-aware (по model id, не по wire API): Anthropic `11on16-bw`, Google `8on22-bw@2048`, OpenAI `8on22-bw`, CJK → `silver16-bw`.
- Bounded source + frames в `CompactionEntry.preserveData.snapcompact`; ре-рендер из bounded source, не перенос PNG.
- `FRAME_TOKEN_ESTIMATE=5 024`, `MAX_FRAMES_DEFAULT=80`, foveation HQ/LQ/HQ.
- Нормализация: ANSI strip, fold newline-runs в `U+2588` FULL BLOCK, dim spans `U+000E/U+000F` для tool-output.
- Требует vision-модель (`model.input` содержит `"image"`).

---

## 5. НОВЫЕ находки (чего не было в известном наборе)

Уже были известны: hashline/replace/patch/apply_patch/sloppy, LSP client/tool/ledger/writethrough, pi-ast/pi-diff/pi-walker/pi-vfs. Ниже — то, что добавляет новую ценность:

1. **pi-diff без единой зависимости** (1020 LOC, 0 unsafe, нет deps в `Cargo.toml`) — jsdiff v9-совместимый Myers с сохранением tie-breaking и change coalescing; `intern`+`myers_diff` для line-shift mapping. Идеальная замена `pa-core/src/tools/jsdiff.rs`.
2. **pi-ast::block — block-range resolver** (`block.rs:60 block_range_at`): по 1-indexed строке возвращает span outermost named node, *начинающегося* на строке; обходит zero-width separator-ноды (tree-sitter-swift), climb до outermost с остановкой на statement-sequence контейнерах (`is_statement_sequence`). Плюс `enclosing_block_boundaries` — generalization «show matching bracket» на tree-sitter с O(window), а не O(nodes) (`block.rs:308`).
3. **Bounded parse cache** (`pi-ast/parse_cache.rs`): `MAX_ENTRIES=12`, `MAX_ENTRY_SOURCE_BYTES=4 MiB`, hash-ключ + **byte-for-byte verify** source (collision = re-parse, не wrong tree). Кэшируется `Tree` (cheap clone, refcount), не результат.
4. **pi-ast structural summary** (`summary.rs:163 summarize_code`): BFS-unfold elidable spans (outer→inner) до target visible lines с hard ceiling; `parsed` флаг через tree-sitter errors. Это то, что даёт «folded summary» в read и, как следствие, seen-lines guard.
5. **WASM grammars on demand** (`pi-ast/language/grammar.rs`): общие грамматики линкуются, остальные (60+ языков) — `.wasm.zst`, скачиваются в `grammar_dir` при первом использовании; lexing в wasm, парс-таблицы копируются в native.
6. **ast-grep strictness ladder** (`pi-ast/ops.rs`): `Cst|Smart|Ast|Relaxed|Signature|Template` + JSON member-fragment auto-wrap fallback (`"key": $V` → wrap в `{...}` и selector `pair`) + Rust contextual pattern wrapper (`fn __rwp_wrapper(){ PATTERN; }`).
7. **`apply_edits` с dedup идентичных правок** (`ops.rs:297`): byte-identical edits (same span+replacement) сворачиваются в одну, а не триггерят overlap; divergent overlap → ошибка.
8. **pi-walker shared scan-cache с env-тюнингом** (`cache.rs:125-132`): `FS_SCAN_CACHE_TTL_MS=1000`, `FS_SCAN_CACHE_MAX_ENTRIES=16`, `FS_SCAN_CACHE_MAX_BYTES=64 MiB`; ranked collection (`WalkRank`), heartbeat каждые 128 записей.
9. **pi-vfs инъектируемая ФС с URL-preserving лексикой** (`provider.rs FileSystem` trait): `is_native_local(path)` — сильная capability (host fast-path разрешён); URL-сегменты percent-encoded; `block_in_place + Handle::block_on` для sync-facade над async-provider.
10. **pi-natives `CancelToken::heartbeat()`** (`task.rs`): кооперативная отмена для CPU-bound задач через libuv thread-pool; `catch_unwind` перед napi `extern "C"` (panic → rejected JS Promise, не abort).
11. **pi-natives tokens.rs**: zstd-embedded tokenizer-словари (o200k/cl100k/ClaudeV3/V47/V5/Qwen3/DeepSeekV3/KimiK2/Glm5/Jev) — offline count_tokens; считает прямо по UTF-16 code units JS-строки.
12. **pi-natives vectors.rs**: batch numeric kernels для mnemopi recall — bit-identical к TS (тот же порядок накопления, non-finite handling, tie-breaking).
13. **pi-edit streaming preview** (`session.rs:254`): `ArgStream` терпимо парсит незавершённый JSON (`close unterminated strings/arrays/objects`), `strip_trailing_unbalanced_removal` не даёт добавлениям «догонять» при streaming; generation-нумерация + final untrimmed pass.
14. **pi-edit `inspect()` без чтения ФС** (`engine.rs:225`): статическая проекция payload (paths, added-lines digest, delete/move intents) для TTSR-матчеров и ACP permission-gate.
15. **pi-edit auto-repair parse regression** (`coding-agent/src/edit/auto-repair.ts`, 417 LOC): локализует минимальный hunk-set, чей revert восстанавливает parse (singles → pairs ≤24 → greedy peel), отдаёт регион smol-модели, принимает кандидат только если файл снова парсится и это не чистый revert; realign indentation против source. Медиана culprit ~14 строк, 94% <150.
16. **LSP inline/deferred diagnostics split** — edit не блокируется на медленном сервере (inline 500 ms, deferred 12 s).
17. **LSP mux per-project daemon** с SHA-256 server-identity и idle-retention процессов для reuse.
18. **pi-iso** — единый PAL изоляции (APFS/btrfs/ZFS/reflink/overlayfs/ProjFS/git-worktree) + `diff` делегирует `git diff` для byte-identical `git apply`.
19. **pi-vcs** — git на `gix` (gitoxide) + jj на `jj-lib`, без subprocess (кроме network).
20. **Bash output minimizer** (`pi-shell/src/minimizer/`, 25 filters: cargo/go/git/bun/docker/…): сжимает stdout/stderr до JS, сохраняет original как `artifact://`.

---

## 6. Топ-10 для переноса + опасности

Оценка под prime: Rust harness, native tool = `python_repl`, программные tools; план `pa-lsp`, `pa-graph`, edit-гибрид; `unsafe_code="forbid"`.

| # | Перенос | Ценность | Опасность/цена |
|---|---|---|---|
| 1 | **`pi-diff` целиком** как `pa-diff` | 1020 LOC, 0 unsafe, 0 deps, Myers + line/word/structured-patch; заменяет `jsdiff.rs` | низкая; переписать тесты parity |
| 2 | **`pi-ast::parse_cache` + `language/grammar`** (bounded tree cache + wasm-on-demand) | фундамент `pa-graph`/`pa-lsp`/edit; 12-слотный кэш, verify-by-bytes | wasm требует `tree-sitter` с feature `wasm` + wasmtime; без него только linked grammars |
| 3 | **`pi-ast::block` (block_range_at + enclosing_block_boundaries + node_chain_at)** | синтаксический block-anchor и bracket-context без эвристик | нужны грамматики; `is_statement_sequence` — список kinds завязан на версии грамматик |
| 4 | **`pi-ast::summary`** (BFS-unfold structural summary) | дешёвое сворачивание больших файлов + seen-lines provenance | логика elision требует аккуратного порта; зависит от грамматик |
| 5 | **pi-edit `EditStore`** (snapshot tags, seen-lines, no-op guard, clipboard) | готовый session-scoped provenance-слой для edit-гибрида | 0 unsafe, но `parking_lot`; prime может взять `std::sync` |
| 6 | **pi-edit seen-lines guard + mismatch diagnostics + no-op limit** | резко снижает правки «по свёрнутому выводу» и stale-правки | требует, чтобы read/search *записывали* seen-lines (provenance); без этого guard не работает |
| 7 | **pi-edit `Session`/`ModeEngine`/`Streaming preview` контракт** | чистый trait-разделение preview/stage/write; streaming UX | `EditWriter` async-trait; prime — tokio, ок |
| 8 | **LSP: inline/deferred diagnostics split + freshness (minVersions) + ledger dedup** | edit не висит на tsserver; не повторяет диагностики | в Rust сложнее: нужен version-tracking по `publishDiagnostics` |
| 9 | **LSP: `defaults.json` (57 серверов) + root-marker/PATH auto-detect + per-model class fallback** | сразу рабочий `pa-lsp` без ручной конфигурации | лицензии/версии серверов; в Rust — свои `lsp-types` |
| 10 | **Bash output minimizer (25 filters) + artifact-offload** | снижение токенов на типовых командах | минимизатор завязан на pi-shell; портировать только filter-таблицу |

### Сводные опасности
- **`unsafe_code="forbid"` (prime Cargo.toml:49)**: `pi-edit`, `pi-ast`, `pi-diff` — 0 unsafe (переносимы как есть). `pi-vfs` (38), `pi-walker` (25), `pi-vsc` (1), `pi-shell` (41) содержат unsafe (`OsStr::from_encoded_bytes_unchecked`, libc mkfifo) — потребуют переписывания на safe API. `pi-natives` (405+) несовместим (N-API + FFI).
- **Зависимости**: pi-edit тянет `xutf`, `xxhash-rust`, `parking_lot`, `regex`, `pi-walker` (транзитивно pi-vfs+rayon). pi-ast тянет `ast-grep-core`, `tree-sitter*` (17 грамматик), `phf`, `globset`, `ignore`. Нужно вычистить лишнее.
- **Строки ошибок — контракт**: `error.rs:1-5` — сообщения байт-в-байт совпадают с TS, «models are trained on them». При порте нельзя переформулировать.
- **seen-lines требует двусторонней интеграции**: без записи provenance из read/search guard бесполезен и может ложно отклонять валидные правки.
- **Tree-sitter как жёсткая зависимость**: block-ops, summary, auto-repair, syntax probe (boundary repair) — всё висит на грамматиках; отсутствие грамматики деградирует до lexical fallback (`enclosing_block_boundaries` возвращает `None` → bracket-scan).
- **LSP-подсистема — 9 850 LOC TS**: порт целиком нецелесообразен; в prime по плану `native-rust-lsp-plan.md` используется `async-lsp` + `lsp-types` (уже в cargo-кэше). Из omp брать *поведенческие* контракты (бюджеты, freshness, dedup, lazy/shared), а не код.
- **snapcompact/pi-predict/pi-voice/pi-iso** — вне текущего фокуса prime (vision-модели / ML / аудио / sandbox-PAL); рассматривать только при появлении соответствующей продуктовой задачи.
- **Нет прямого аналога graph-БД**: в omp «граф кода» фактически = tree-sitter AST + ast-grep + episodic-graph в mnemopi (SQLite, `core/episodic-graph.ts`). Для `pa-graph` (SQLite) — брать идеи из mnemopi `episodic-graph.ts`/`triples.ts`, а не из крейтов.
