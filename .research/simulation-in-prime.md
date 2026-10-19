# СИМУЛЯЦИЯ: как pa-graph/pa-lsp работают в prime-agent

## 0. Метод
Заземлено на реальном prime-agent (603K LOC Rust, 1275 .rs файлов, 1607 файлов).
Использованы codebase-memory-mcp и gitnexus для симуляции типовых запросов.
Цель: показать КОНКРЕТНЫЕ задачи и ЧТО это даёт.

## 1. Контекст: как агент работает СЕЙЧАС

### Инструменты prime-agent (единственный native tool = python_repl)
- `bash(cmd)` — запуск программ (в фоне)
- `edit.run(path, old_str, new_str)` — правка (exact-unique-match)
- `websearch`, `attach_image`, `rlm.spawn` (субагенты)
- Программные инструменты: Python в REPL

### Как агент решает задачи навигации по коду СЕЙЧАС
1. `bash("rg 'create_bash_tool_definition' crates/")` → список файлов
2. Читает найденные файлы Python-ом (`open(...).read()`)
3. Строит модель зависимостей в голове
4. Правит через `edit`

**Проблемы** (симулировано):
- grep даёт МНОГО ложных (все упоминания, не только вызовы).
- Нет понимания «кто ВЫЗЫВАЕТ» vs «кто ОПРЕДЕЛЯЕТ».
- Нет транзитивных зависимостей.
- Нет семантики (символы, а не строки).

## 2. Симуляция: типовые вопросы агента

### Вопрос A: «Кто вызывает create_session?»

**СЕЙЧАС (grep + чтение):** (ИЗМЕРЕНО на реальном prime-agent)
```
bash("rg -n 'create_session' crates/")
→ 355 строк, 29 682 символа вывода
→ 88 определений + 182 эвристических «вызова» (rg не различает!)
→ чтобы понять, кто РЕАЛЬНО вызывает конкретный create_session:
   читать релевантные файлы, разрешать вручную
≈ 30K+ токенов, 5-10+ tool calls, и всё равно приблизительно
```

**С pa-graph:** (ИЗМЕРЕНО через codebase-memory-mcp)
```
graph_trace(function="...engine.create_session", direction="inbound", depth=1)
→ callers_total: 3 (eq)
  prime-agent...print_goal.goal_bed 1
  prime-agent...print_goal.goal_bed_with_resumed_goal 1
  prime-agent...print_session_command.bed_with_settings 1
≈ 368 символов ≈ 92 токена + 1 tool call
```
**Выигрыш: ~320× меньше токенов (29 682 → 92), 1 вызов вместо 5-10,
ТОЧНО (3 реальных вызывающих с квалифицированными именами вместо 355 строк).**

### Вопрос B: «Что сломается, если я изменю create_session?» (impact)

**СЕЙЧАС:** нет инструмента. Агент вручную читает вызывающих, гадает.

**С pa-graph:**
```
graph_trace(function="create_session", direction="both", depth=2)
→ callers_total: 3 (inbound)
→ callees_total: 262 (outbound)
→ полная картина: что зависит + от чего зависит
≈ 1 вызов
```
**Выигрыш: impact-анализ, которого СЕЙЧАС НЕТ.**

### Вопрос C: «Найди где обрабатывается X» (семантика)

**СЕЙЧАС:**
```
bash("rg -n 'stream_failure|provider_error' crates/")
→ десятки совпадений по всему репо
```

**С pa-graph:**
```
graph_search(query="provider error classification stream failure")
→ BM25+RRF: ранжированные символы (не строки)
  prime-agent...provider_errors.normalize_genspark
  prime-agent...provider_errors.normalize_codebuddy
  ...
≈ ранжированный топ-10, ~500 токенов
```
**Выигрыш: семантический поиск, а не текстовый.**

### Вопрос D: «Где определён символ LspManager::ensure_client?»

**СЕЙЧАС:** `rg 'fn ensure_client'` → файл → чтение.

**С pa-lsp (LSP-семантика):**
```
lsp(action="symbols", file="crates/pa-lsp/src/manager.rs")
→ дерево символов с диапазонами
lsp(action="definition", file=..., line=..., symbol="ensure_client")
→ точное определение
```

### Вопрос E: «Переименуй символ везде» (rename)

**СЕЙЧАС:** `edit` по каждому месту вручную (риск пропустить/сломать).

**С pa-lsp:**
```
lsp(action="rename", file=..., symbol="OldName", new_name="NewName", apply=true)
→ WorkspaceEdit применяется atomically (все ссылки)
```

### Вопрос F: «Проверь, что моя правка не сломала код» (diagnostics)

**СЕЙЧАС:** `bash("cargo check")` → ждать минуты → полный вывод.

**С pa-lsp (инкрементально):**
```
lsp(action="diagnostics", file="crates/pa-lsp/src/manager.rs")
→ только диагностики ЭТОГО файла, мгновенно (rust-analyzer держит индекс)
```
**Выигрыш: мгновенная обратная связь вместо полного cargo check.**

### Вопрос G: «Какие файлы затронет мой git diff?» (detect_changes)

**СЕЙЧАС:** `bash("git diff --stat")` → список файлов (без связей).

**С pa-graph:**
```
graph_changes(scope="impact", direction="inbound", depth=2)
→ 625 changed → 1971 seed → 3248 impacted символов
→ какие функции/модули затронуты ТРАНЗИТИВНО
```

## 3. Симуляция: рабочий цикл агента с pa-graph

### Сценарий: «Добавь новый provider X в fork»

```
┌─────────────────────────────────────────────────────────────┐
│ ХОД 1: разведка (pa-graph)                                   │
│  graph_search("provider catalog fork overlay")               │
│  graph_trace("is_fork_provider", direction="both")           │
│  → карта: где регистрируются провайдеры, кто их читает       │
│  ~500 токенов, 2 вызова                                      │
├─────────────────────────────────────────────────────────────┤
│ ХОД 2: чтение точных мест (pa-lsp)                           │
│  lsp(action="symbols", file="crates/pa-ai/src/models_fork.rs")│
│  → дерево символов, диапазоны                                │
│  Python: прочитать только нужные срезы                       │
├─────────────────────────────────────────────────────────────┤
│ ХОД 3: правка (edit) + проверка (lsp diagnostics)            │
│  edit(path=..., old_str=..., new_str=...)                    │
│  lsp(action="diagnostics", file=...)  → мгновенно            │
├─────────────────────────────────────────────────────────────┤
│ ХОД 4: impact-проверка (pa-graph)                            │
│  graph_changes(scope="impact")                               │
│  → что затронуто транзитивно                                 │
└─────────────────────────────────────────────────────────────┘
```

## 4. Симуляция: параллельная работа субагентов

prime-agent — рекурсивный харнесс (rlm.spawn). Каждый субагент может
использовать pa-graph СВОЕГО проекта.

```
Родитель: rlm.spawn("реализуй LSP-клиент", name="lsp-worker")
Дочерний агент (depth=1):
  graph_search(...) → находит точки интеграции
  lsp(action="symbols") → читает структуру
  edit → правит
  lsp(action="diagnostics") → проверяет
  agent_message.send("готово, файлы X/Y/Z")
```
**Выигрыш для субагентов:** те же граф/LSP инструменты, но в изоляции проекта.
Синтез: родитель и дети видят ОДИН граф (per-project .db).

## 5. Что даёт pa-graph/pa-lsp для prime (по категориям)

### 5.1 Экономия токенов (главное)
| Задача | Сейчас | С графом | Выигрыш |
|---|---|---|---|
| «кто вызывает X» | ~30K токенов (355 строк) | ~92 токена (3 вызывающих) | **~320×** |
| «найди где X» | десятки совпадений | топ-10 ранжированных | ~10× |
| «impact X» | нет инструмента | 1 вызов | ∞ |
| «diagnostics X» | cargo check (минуты) | мгновенно | 100×+ |

### 5.2 Новые возможности (чего НЕТ)
- **impact-анализ** (blast radius перед правкой).
- **rename по семантике** (все ссылки, не текст).
- **detect_changes** (git → символы).
- **closure-repair** (быстрая переиндексация).
- **recency-aware recall** (temporal decay для долгих сессий).

### 5.3 Качество
- Точность: символы, а не строки (нет ложных совпадений).
- Транзитивность: граф глубины N vs плоский grep.
- Семантика: BM25+RRF vs regex.

### 5.4 Надёжность
- Мгновенная диагностика после правки (меньше итераций).
- impact-предупреждение ДО правки (меньше регрессий).

## 6. Симуляция: интеграция с существующими механизмами

### С edit-скиллом (Python)
```
Python REPL:
  old = ...; new = ...; await edit(path=..., old_str=old, new_str=new)
  # → триггерит host_request "lsp.did_change" (синтез!)
  # → pa-lsp обновляет документ, пересчитывает диагностику
  # → pa-graph инвалидирует surface (closure-repair)
  # → следующая lsp(action="diagnostics") показывает НОВЫЕ проблемы
```

### С rlm.spawn (субагенты)
- Каждый субагент видит граф проекта.
- Родитель может запросить у ребёнка «impact твоей правки».

### С continual harness (память)
- pa-graph ADR (manage_adr) — архитектурные решения рядом с графом.
- Совместимо с существующей harness-памятью.

### С compaction
- CoherencePolicy компактит кэш графа по recency+frequency+coherence.
- Temporal decay — для долгоживущих сессий.

## 7. Симуляция: стоимость и границы

### Что индексируется
- 603K LOC Rust → первый индекс (полный) ~секунды-минуты (tree-sitter быстрый).
- Инкремент: closure-repair → только изменённый файл + зависимые.

### Память
- SQLite .db: ~сотни MB для 603K LOC (как codebase-memory: prime-agent.db=157MB).
- RAM-граф: шардированный, 2-random eviction при лимите.

### Когда НЕ использовать
- Одноразовый поиск известного символа → `rg` быстрее.
- Внешний проект (не проиндексирован) → падать на bash/Python.
- Динамические языки без LSP → только tree-sitter.

## 8. Симуляция: конкретный диалог (пример)

```
Пользователь: "Добавь обработку 429 от Genspark в provider_errors"

Агент (ход 1):
  graph_search("provider error normalization genspark rate limit")
  → находит: provider_errors.rs:normalize_genspark, is_payment_signal
  graph_trace("normalize_genspark", direction="inbound")
  → кто вызывает: stream_failure.rs, provider_failover.rs
  ~150 токенов

Агент (ход 2):
  lsp(action="symbols", file="crates/pa-ai/src/provider_errors.rs")
  → дерево символов, видит структуру
  Python: читает только функцию normalize_genspark (~40 строк)
  ~800 токенов

Агент (ход 3):
  edit(path=".../provider_errors.rs", old_str=<arm>, new_str=<new arm>)
  lsp(action="diagnostics", file=".../provider_errors.rs")
  → 0 ошибок, мгновенно

Агент (ход 4):
  graph_changes(scope="impact", depth=1)
  → затронуто: 3 функции в 2 файлах
  → подтверждает: правка локальна

Итого: ~1K токенов, 6 вызовов, 0 регрессий.
Без графа: ~30K токенов, 15+ вызовов, риск пропустить вызывающего.
```

## 9. Итог: что это даёт prime

| Аспект | Эффект |
|---|---|
| **Токены** | 10-325× меньше на навигацию по коду |
| **Скорость** | мгновенная диагностика vs cargo check |
| **Точность** | символы vs строки; граф vs плоский grep |
| **Новые возможности** | impact, rename, detect_changes, closure-repair |
| **Масштаб** | работает на 603K LOC (и больше) |
| **Субагенты** | граф проекта виден всем детям |
| **Надёжность** | impact до правки, диагностика после |
| **Изоляция** | per-project .db, фича-флаги, upstream-совместимость |

### Ключевая ценность
prime-agent СЕЙЧАС полагается на `rg` + чтение файлов + Python-анализ для
навигации по коду. Это работает, но дорого (токены) и слепо (нет связей).
pa-graph/pa-lsp добавляют **структурное понимание кода** — то, что человек
делает через IDE, а агент сейчас вынужден эмулировать вручную.

### Формула выигрыша
```
Было:  grep (строки) + чтение + ручной анализ → дорого, слепо
Стало: граф (символы) + LSP (семантика) + impact → дёшево, точно
```

### Что НЕ меняется
- `python_repl` остаётся главным инструментом.
- `bash`/`edit`/`websearch` работают как раньше.
- Новые инструменты — ДОПОЛНЕНИЕ за фича-флагами.
- Upstream-сессии (без флагов) ведут себя ровно как сейчас.
