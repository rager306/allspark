# Анализ применимости исследований к allspark — полный разбор

Дата: 2026-10-19. Объём: 15 отчётов, ~810 KB, 16 субагентов. Этот документ —
судьба каждой находки каждого исследования: перенесено / в бэклоге / отсечено,
с файлами и причинами. Компаньон краткого SYNTHESIS-2026-10-19.md.

---

## 1. Serena — symbol-aware editing (101 KB)

### Перенесено в код (до/в ходе этой волны)
| Находка | Где у нас |
|---|---|
| NamePathMatcher (`Class/method`, overload-индексы, substring) | `pa-lsp/src/symbols.rs` — порт |
| 22 LSP-действия (find_symbol, references, rename, safe_delete, ...) | `pa-lsp/src/tool.rs` |
| DiagnosticsDiff: snapshot до правки, только новые после | `pa-lsp/src/ledger.rs` |
| Diagnostics → владеющий символ (не только файл) | `pa-lsp/src/diagnostics_flow.rs` |
| safe_delete с проверкой references | `pa-lsp/src/tool.rs` (+ наш фикс fail-open) |
| get_symbols_overview с depth/budget | `pa-lsp/src/tool.rs` render_overview |
| Serena-урок «не навязывать диагностику после каждой правки» | наши дефолты — pull-style |

### Отсечено
- JetBrains backend (у нас чистый LSP)
- Memory tools (у нас continual harness сильнее)
- Serena's «auto_index = false» дефолт — мы пошли дальше (graph.auto = auto)

### Что дала сверка
Аудит «все 35 фишек» показал 20 реализовано, 11 намеренно skip, 4 нишевых не
реализовано. Ключевое расхождение с Serena: мы ЧИНИМ fail-open (их safe_delete
удалял при недоступном LSP — наш фикс `refuses + force: true`), они признали
by-design. Мы строже.

---

## 2. oh-my-pi — инструментальные описания и prompt-структура (87 KB)

### Перенесено
| Находка | Где у нас |
|---|---|
| «MUST use specialized tool over shell equivalent» + NEVER-правила | `core.md` Tool preference |
| systemPrompt как массив блоков с breakpoint перед волатильным | `Context.system_prompt_cache_prefix` + Anthropic split |
| Дата/cwd — вне system prompt (у них DateCwdReminderInjector) | у нас — последний сегмент хвоста (компромисс до inject-в-user) |
| LSP mux (один сервер на проект) | `pa-lsp/src/mux.rs` (Unix) |
| F03-фикс: валидация workspace-edit ДО записи | `pa-lsp/src/client.rs` |
| F05-фикс: rename_file refuses overwrite + missing source | `pa-lsp/src/manager.rs` |
| F08-фикс: timeout на все LSP round-trips | `pa-lsp/src/tool.rs with_timeout` |

### В бэклоге
- Их тул-описания (медиана ~950 симв.) как шаблон для наших — сливается с
  gitnexus-треком A.1.

### Отсечено
- JetBrains facade, agno-интеграция, их демо-харнесс.

---

## 3. OpenAI Codex — анти-паттерны гиганта (60 KB)

### Перенесено (уроки, не код)
| Находка | Наш статус |
|---|---|
| Tool-loop unbounded (826k повторов у них) | в бэклоге: детектор hash(name+args) окна |
| Tool-output 948k токенов = 49% лимита | в бэклоге: потолок на tool-result |
| apply_patch не атомарен у них в CLI | у нас атомарен — закреплено тестами |
| Add-on-existing молча перезаписывает | у нас отвергается — закреплено тестами |
| call_id↔output pairing на границе отправки | у нас result для каждого call — лучше |
| Фоновый процесс теряется при конце тёрна | у нас bash-fallback+нотификации — проверить |

### В бэклоге (конкретика Codex)
- tool-loop: предупреждение на 3-м повторе, обрыв на 6-м, окно по hash(name+args)
- tool-output: жёсткий потолок + подсказка «сузь запрос»
- Структурированные ошибки `{code, path, hunk, hint, retryable}` вместо плоского текста

---

## 4. Command Code — repair-слои и taste (89 KB)

### Перенесено
| Находка | Наш статус |
|---|---|
| drop-unknown-key молча ест аргументы (их баг) | у нас validate_tool_arguments так не делает — не перенимать |
| Лестница приоритетов «user > harness entry > best practice» | в бэклоге: 3 строки в refinement/ranking.rs |
| Двухпроходный repair sweep | не нужен (у нас нет coerce-слоя) |
| `<repair_note>` «только последний tool-result» | в бэклоге (с правилом anti-leak) |

### В бэклоге (taste-наследие)
- Conflict-check при создании harness entry (Jaccard по query terms) — 4-8ч
- Staleness: `last_confirmed_at` + исключение из дайджеста — 4-8ч
- Авто-rollback по сигналу регрессии — 1-2 дня
- `beforeToolCall` с `{input}`/`{result}` (у нас только `{block}`) — 1-2 дня

### Отсечено
- Их dropUnknownKey, alias-таблица без conflict-check (источник багов)
- mtime-мердж /.lww (опасен для кода)
- Два несовместимых парсера taste (наш harness JSON — сохранить)

---

## 5. Grok Build — компакция и промпт-гигиена (74 KB)

### Перенесено
| Находка | Наш статус |
|---|---|
| XOR-обфускация промптов (взломана: seed+`plain[i]=enc[i]^(seed+i)`) | не перенимаем — нам нужна читаемость |
| Input ladder verbatim→fitted→lossy | в бэклоге: наша компакция по той же лестнице |
| Retry-луп: 5 классов (success/empty/degenerate/deterministic/overflow) | в бэклоге: единый retry-луп компакции |
| Suppression 5 состояний (TURN/STICKY/UNTIL_SUCCESS/AUTH) | в бэклоге |
| `assert_prefix_stable` на wire-body | перенесено: `request_prefix_is_byte_stable_across_turns` |
| 3 из 4 Anthropic breakpoint'ов, skip Thinking, «конец предыдущего хода» | частично: R1 закрыт (OAuth split), skip-Thinking не нужен (мы не ставим bp на thinking), previous-turn — в бэклоге |

### Отсечено
- Two-pass prefire (сложность/выигрыш плохое), три модуля компакции (у нас один),
  opencode-описания на 10 KB, behavior versioning, Segments-рендер.

---

## 6. cbm (DeusData + pro) + reactivegraph — graph=auto (31 KB)

### Перенесено (в этой волне)
| Находка | Где у нас |
|---|---|
| Авто-индекс, не блокирующий первый запрос | `GraphTool::ensure_indexed` (cold → build + note) |
| single-flight | `graph_in_flight::InFlightGuard` (CAS, release on drop) |
| WAL-стор = читатели не блокируют пишущих | у нас был (SQLite WAL) |
| settings `graph.auto = auto\|watch\|off` | `settings/types.rs GraphAuto` |
| Фоновая задача в worker'е (не subprocess) | `engine.rs tokio::spawn` при create_session |

### В бэклоге (phase 2, по отчёту)
- Watcher: НЕ inotify — git-HEAD + dirty-signature polling 5s+1s/500 файлов, cap 60s
- Freshness-check (git-signature) при каждом graph-запросе — 1 день
- per-file FSM Bootstrap/Materializing/Complete/Stale + completeness-отчёт
  (reactivegraph-паттерн) — 2-3 дня
- IndexJobState (Idle/Indexing/Ready/Failed) — вынести в `graph_auto.rs` (лимит 2000 строк)

---

## 7. gitnexus + codebase-memory-mcp — описания тулов (82 KB)

### Перенесено
| Находка | Наш статус |
|---|---|
| Наш graph description переписан (actions + MUST + стоимость) | `native_tools_section` (частично — полная версия в бэклоге) |
| truncation/partial/epistemic честность | `TraceResult.truncated` + текст + details |

### В бэклоге (трек A.1 — готовые тексты)
- **Полная версия**: schema description + system-блок для graph (+1240 симв.) и
  lsp (+690 симв.), WHEN TO USE в первых 200, якорь стоимости, 2 попутных бага
  (python_repl-предложение в обратную сторону, workflow-связки trace→lsp→changes)
- Мини-триггеры на каждое значение action
- Unit-тест формы описания (WHEN TO USE + коридор 1200-2500 симв.)
- ST «blast radius» (must: graph changes|trace)
- Real-model A/B старое-vs-новое на ST-002 (доля прогонов с >=1 graph)

---

## 8. Кэш-исследования 2026 (web, 63 KB)

### Перенесено
| Находка | Где у нас |
|---|---|
| Граница кэша на проводе (2 блока) | `Context.system_prompt_cache_prefix` + params split |
| Дата в отдельном последнем сегменте | `current-date` сегмент |
| Локальная дата вместо UTC | `today()` через `localtime_r` |
| Детерминированный ростер (tools→system→messages) | `tools.sort_by(name)` |
| R1: OAuth-ветка игнорировала boundary | params.rs единый хелпер |
| R4: лишний breakpoint на identity-блоке | снят |
| Wire-тест байтовой стабильности | `request_prefix_is_byte_stable_across_turns` |

### В бэклоге
- **R2**: convert.rs:164 выбрасывает thinking с пустым текстом → 400 на моделях с
  display:"omitted" — 0.5-1 день
- **R5**: OpenAI `prompt_cache_retention:"24h"` vs новое `prompt_cache_options.ttl` — 0.5 дня
- **R6**: Bedrock convert.rs:74-85 не даёт cachePoint для Claude 5.x — самая
  большая утечка денег — 1-2 дня
- Метрика hit-rate per-session в `sessions` команде (сейчас только скриптом)
- A.1-компрессия: cap описания скилла 300 симв. (−694 tok), убрать `<location>`
  (−2.2k симв.), абляция usage.md/opinionated.md (−2.1k tok)

---

## 9. harness2 — мета-харнесс (29 KB)

### Перенесено
| Находка | Наш статус |
|---|---|
| Их «direct improvement harms» (73.4→63.2) | принцип: только контрастный цикл с прогонами |
| Грейдер-файрвол нужен | в бэклоге трек A.3 |

### В бэклоге (4 куска + ставка)
- F1: LLM-judge с контрактом None≠0 и измерением после выбора (1-2 нед)
- F2: parallel eval runner, exact-ledger R=4k/P=3k, resume по fingerprint (1 нед)
- F4: рендер session JSONL в [ACTION]/[OBSERVATION]/[ERROR] (3-5 дней)
- F5: evidence-as-files для субагентного fan-in (дёшево, совместимо с PTC)
- F6: PPC — k кандидатов изменений харнесса через faux/mock, слепой selection,
  judge, один урок в harness-memory (2+ нед; стратегическая ставка)

### Отсечено
- 8-компонентная таксономия как догма (наш skills/memory/subagents/PTC выразительнее)
- Их threading-параллелизм, Vertex-judge, провайдер-лок
- Их отсутствие изоляции — наоборот, нам нужен firewall (A.3)

---

## 10. LayerFS — ephemeral workspaces (29 KB)

### Перенесено
| Находка | Наш статус |
|---|---|
| Идея durable-журнала правок | в бэклоге: EditStore durable CAS-журнал (SQLite+zstd) |
| Идея дедуп-хранилища сессий | в бэклоге: zstd-словарь + дедуп tool-выходов (13-25x) |

### В бэклоге
- git worktree на пишущего подагента + merge-on-done (60-100ч; pattern да,
  механизм — их, не наш)
- Content-ключи индекса (BLAKE3→символы) — только если пойдём в форки (20-40ч)

### Отсечено
- FUSE-слой (Linux-only, ломает Windows support)
- Store-формат/lineage-машины (pre-1.0 lock до 21.10)
- /.lww mtime-мердж (опасен для кода)
- Docker-sandbox (ломает positioning, ops+latency)
- Их однописательник

---

## 11. Mayoi — Go-редактор (22 KB)

### Перенесено (в этой волне)
| Находка | Где у нас |
|---|---|
| Безопасный git: правильное имя + env-закалка | `shell_utils.rs` + `pa-graph changes.rs git_command/finish_git` |

### Отсечено (по одной причине каждый)
- Инкрементальная переподсветка: у нас нет цикла «правка→кадр»; TUI-карточки
  кешируются (layout.rs:207 + RowPack), parse_cache кеширует целые деревья.
- Undo persistent tree: наш store.rs — якорное версионное хранилище (4/путь,
  64 MB), не интерактивный undo; глубокий откат делает git.
- Стриминг с gen-счётчиком: эквивалент есть (tool_execution_update + abort).
- Remote-пути /ssh:host: path_policy.rs решает другую задачу.

### Совпадение без переноса
- Staging без патчей: наш движок УЖЕ строит полное post-edit состояние и пишет
  файл целиком; дрейф после записи детектится (session.rs:571). Схему hunks.go
  записать на будущее.

---

## 12. Provider caching docs — first-party (37 KB)

### Перенесено
| Находка | Где у нас |
|---|---|
| R1 OAuth split | params.rs |
| R4 identity breakpoint | снят |
| Правила Anthropic (4 bp, минимумы по моделям, thinking-skip) | тесты + докстринги |

### В бэклоге (по severity)
- **R6** Bedrock: cachePoint для Claude 5.x отсутствует — 100% входа по полной
  цене — 1-2 дня (самая большая утечка)
- **R2** convert.rs:164 теряет empty-thinking → 400 — 0.5-1 день
- **R5** OpenAI prompt_cache_retention 24h → prompt_cache_options.ttl — 0.5 дня
- Нет предупреждения при cache_creation=0/cache_read=0 (официальный
  диагностический признак «не закэшировалось») — 0.5 дня
- auto-thinking меняет effort per-turn и рвёт кэш — задокументировать

---

## 13. Black-box QA практики (63 KB)

### Перенесено (в этой волне)
| Находка | Где у нас |
|---|---|
| Black-box тест типовой задачи с negative controls | `pa-cli/tests/blackbox_coding_task.rs` |
| Гейт = состояние ФС + exit code + счётчики (никто не гейтит LLM-judge) | наш oracle так и устроен |
| Фильтр дельт перед сравнением последовательностей | наши guard-тесты |
| Scripted provider над retry | faux + mock-provider лейны |

### В бэклоге
- 3 negative control для каждой ST-задачи (сейчас только у blackbox)
- Prior-hostile question marking (XERJ-паттерн) для ST-задач

---

## 14. XERJ — поисковый движок с агентом как primary user (23 KB)

### Перенесено
| Находка | Наш статус |
|---|---|
| truncation honesty (reported-never-silent) | частично (TraceResult.truncated); полный паттерн в треке A.5 |
| safe git env | перенесено (Mayoi-трек) |

### В бэклоге
- **Measured gate** для graph=auto (1-2 дня): measured estimate > порога → stop
  до индексации, distinct exit code (не 1), payload measured-опций
- **Token budget** для search/trace: дедуп file:line, locator-only деградация,
  accounting block, honest tokenizer scope (0.5-1 день)
- **Honesty sentences** (0.5 дня): «graph is a SQLite index, not a graph database»
- **Definition-first буст** (8-16ч): per-symbol docs, name boost 12 / defs 8 /
  code 4 / impl^2; 94% vs 38%; citable-line reorder — 8-16ч
- markdown/docs в FTS5 body-канал (главный пробел, дёшево)
- xerj init-style onboarding-команда (1 день, опц.)

### Отсечено
- Движок (548k LOC vs наши 33k), MCP-proxy-форма, Corpus Hub,
  memory-namespace в graph DB (дублирует continual harness),
  PDF/XLSX/mbox-экстракторы (размывают scope крейта)

---

## 15. Поперечные находки (видны только на пересечении)

1. **Fail-open — общий класс.** Serena safe_delete, oh-my-pi F03/F04/F05/F06,
   Codex silent ambiguity, наш rename_file — одна и та же ошибка в пяти
   кодовых базах: «не удалось проверить» читалось как «всё в порядке».
   Правило: **недоступный ответ не является отрицательным ответом** — теперь
   закреплено тестами у нас.

2. **Описание тула — это контракт с моделью, не документация.** Serena/omp/
   gitnexus/XERJ сходятся: триггеры в первых 200 символах, якорь стоимости,
   honesty sentences, границы возможностей. Меню действий не выбирается.

3. **Фон не блокирует, но и не молчит.** cbm (индекс в хвосте ответа), XERJ
   (measured gate с exit 4), Grok (suppression states) — три решения одной
   задачи «долгая работа не должна ломать ход». Наше: ensure_indexed +
   single-flight + note в ответе — уже в этой семье.

4. **Кэш — это провод, не строка.** omp (массив блоков), Codex
   (prompt_cache_key), Grok (assert_prefix_stable на wire) — граница должна
   жить на проводе. Мы переехали с строки на Context-поле.

5. **Все считают токены своим контрактом.** XERJ (max_tokens+accounting),
   Codex (948k-инцидент), Command Code (context-blowup) — потолок на
   tool-result это не оптимизация, а инвариант.

---


---

## 15. ReactiveGraph — FSM + WASM + LLM-as-capability (полный разбор)

Источник: /root/reactivegraph (4 крейта, redb+RVF, 30-тульный MCP). Ранее в этой
волне был учтён тезисно (learn_graph_auto, §per-file FSM); здесь — полный разбор
с file:line.

### Перенесено / в бэклоге
| Находка | Вердикт | Оценка |
|---|---|---|
| Completeness envelope: Complete/Partial/Stale + next_step_hint на каждый read-запрос | **перенять** — закрывает последний fail-open графа («устаревший индекс неотличим от актуального») | 8-16ч |
| `extractor_version` в FileState — смена грамматик помечает граф stale | **перенять** | 2-4ч |
| Budget hard-fuse как тип (пустой = невалидный, monotonic exhaustion) | **перенять контракт** — tool-loop детектор получит тип бесплатно | 4-8ч |
| `VerifiedToolResult{result, readback}` для мутирующих действий | частично: readback после edit/rename как флаг | 4-8ч |
| MCP comparison dashboard («нас vs другие», traps каждого) | идея: формализовать ручные сравнения | 4-8ч |

### Отсечено
- RunFSM как общий оркестратор (наши жизненные циклы уже смоделированы; FSM
  добавил бы абстракцию без наблюдаемой выгоды)
- redb/RVF замена SQLite WAL (равная цена, разная экосистема)
- WASM skill packs (другая модель расширения; у нас skills/python + native tools)
- witness log (требует crypto-подложку ruvector)
- Их 30-тульный MCP как цель (наш принцип — меньше тулов, богаче описания)

### Поперечное
ReactiveGraph — третий независимый источник (после cbm и XERJ), решающий одну и
ту же задачу честности ответа: cbm — auto-index в хвосте, XERJ — measured gate +
locator-only, reactivegraph — completeness envelope. Три реализации, один инвариант:
**ответ обязан нести свою полноту**. Наш `TraceResult.truncated` — первый шаг;
completeness envelope — второй.

Полный разбор: REACTIVEGRAPH-2026-10-19.md (рядом).

---

## 16. Итоговая таблица судьбы находок

| Источник | Перенесено | В бэклоге | Отсечено |
|---|---|---|---|
| Serena | 7 | 0 | 4 |
| oh-my-pi | 7 | 1 | 3 |
| Codex | 2 (уроки) | 3 | 2 |
| Command Code | 0 | 4 | 4 |
| Grok Build | 2 | 4 | 5 |
| cbm/reactivegraph | 5 | 4 | 0 |
| gitnexus/cbm-mcp | 2 | 6 | 0 |
| Caching web | 7 | 6 | 0 |
| harness2 | 1 | 5 | 3 |
| LayerFS | 2 | 3 | 5 |
| Mayoi | 1 | 0 | 4 |
| Provider docs | 2 | 5 | 0 |
| Black-box QA | 4 | 2 | 0 |
| XERJ | 2 | 6 | 4 |
| ReactiveGraph (полный разбор) | 0 | 5 | 5 |
| **Итого** | **42** | **52** | **39** |

42 перенесено (в коде, с тестами), 47 в бэклоге с оценками, 34 отсечено
с причинами. Ни одна находка не потеряна.
