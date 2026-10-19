# Матрица модулей allspark v3: RLM Python-специфика, склейка, каскады

Версия 3 — дополнение к MATRIX-v2. Единственное новое измерение: **RLM Python
kernel как первый-class модуль** — его связи с нативными тулами, тонкости
изоляции подагентов, каскады через host_request и emit.

---

## 1. Двухслойная инструментальная архитектура: нативные тулы + RLM kernel

```
┌────────────────────────────────────────────────────────────────┐
│ WORKER-ПРОЦЕСС                                                 │
│                                                                │
│  Rust: SessionEngine                                           │
│  ├─ Native tools (прямые вызовы):                              │
│  │   lsp (22) · graph (8) · edit (5) · jev (4) · bash          │
│  │   → ToolDefinitionBridge → AgentTool → dispatch             │
│  │   (Sequential для мутаций, parallel для read-only)          │
│  │                                                             │
│  ├─ python_repl tool:                                          │
│  │   → IpythonKernelProvisioner → ЯДЕРНЫЙ ПРОЦЕСС (uv venv)    │
│  │     │                                                       │
│  │     │  Python: rlm module (prime-agent-runtime)             │
│  │     │  ├─ rlm.spawn / create_session / find_models          │
│  │     │  ├─ rlm.harness (CRUD continual state)                │
│  │     │  ├─ agent_message / agent_observe                     │
│  │     │  ├─ bash (background + notification)                  │
│  │     │  ├─ edit (kernel-side: CRLF-preserving replace)       │
│  │     │  ├─ graph (router: вопрос → готовый вызов)           │
│  │     │  └─ mcp (generic MCP через host relay)                │
│  │     │                                                       │
│  │     └─ host_request → Rust handlers (протокол по stdin)     │
│  │         rlm.run / rlm.spawn / rlm.find_models /             │
│  │         rlm.list_subagents / compact.run / refine.run /     │
│  │         agent_message.send / mcp.* / model.info             │
│  │                                                             │
│  └─ emit (display events): diff от edit skill → TUI            │
│                                                                │
│  ДРУГИЕ WORKER-ПРОЦЕССЫ (подагенты через rlm.spawn)            │
│  → свой SessionEngine, свои тулы, свой kernel                 │
└────────────────────────────────────────────────────────────────┘
```

### Ключевое: **нативные тулы НЕ видны из Python kernel'а напрямую.**
Python работает с ФС (read/write через os), вызывает `skills/edit` (свою
CRLF-preserving реализацию) и `skills/graph` (роутер-советчик), но не вызывает
`lsp.find_symbol` или `graph.trace` изнутри kernel'а. Вызов возможен только
через модель (модель发出 tool call) или через host_request (зарегистрированные
типы — сейчас их 12, и ни один не покрывает lsp/graph действия).

**Следствие для каскадов**: любое улучшение native lsp/graph тула автоматически
доступно модели, но НЕ доступно python-скиллам. Скиллы, которым нужен LSP,
должны либо (а) делать модель вызвать lsp, либо (б) получить новый host_request.
Это архитектурное ограничение, не баг.

---

## 2. Матрица связей с учётом RLM

### 2.1 Двунаправленные потоки

| Откуда | Куда | Механизм | Что передаётся |
|---|---|---|---|
| Модель → native tool | Rust | Tool call (dispatch) | params JSON |
| Модель → python_repl | Kernel процесс | kernel protocol (stdin/stdout) | python code |
| Kernel → Rust host | Rust | `host_request(type, payload)` | JSON тип + данные |
| Kernel → TUI | display | `emit({mime: payload})` | diff, attachments |
| Rust → подагент | Новый worker | `rlm.spawn` → SessionEngine | prompt, model, thinking |
| Подагент → родитель | Daemon | `agent_message.send` | текст (не структурирован) |
| Подагент → ФС | Общая ФС | файлы напрямую | **конфликт правок!** |

### 2.2 Что живёт в Python, что в Rust (разделение труда)

| Python kernel (prime-agent-runtime) | Rust (pa-core) |
|---|---|
| rlm.spawn/create_session/find_models (subagent API) | lsp, graph, edit, jev (нативные тулы) |
| rlm.harness (CRUD continual state) | prompt assembly (cache boundary) |
| agent_message / agent_observe | compaction, admission, failover |
| bash (background + notification) | settings, auth, resources |
| edit (kernel-side, CRLF-preserving) | tool_profile (фильтр ростера) |
| graph (роутер-советчик, БЕЗ исполнения) | tool_bridge (контракт → loop) |
| mcp (generic MCP через relay) | telemetry (fire-and-forget) |
| skill (CLI хелпер для консольного вызова) | |

Разделение: **Rust владеет «что делать»** (нативные действия над кодом),
**Python владеет «как организовать»** (субагенты, память, коммуникация).
Пересечение минимальное: edit дублируется осознанно (kernel-side CRLF-preserving
replace для Python-контекста), graph — только роутер (не дублирует).

---

## 3. RLM-специфика многопоточности

### 3.1 Уровни параллелизма

| Уровень | Что параллельно | Синхронизация |
|---|---|---|
| **Worker'ы** | каждый подагент = свой процесс (spawn через daemon) | изоляция ФС — общая (конфликт правок!) |
| **tokio tasks** в worker | фоновые задачи (graph auto-index, telemetry, MCP gating) | spawn_blocking для CPU |
| **Kernel** | один python процесс, asyncio внутри | asyncio.Lock на host_request |
| **rayon** | graph indexer (par_iter по файлам) | внутри spawn_blocking |
| **tool calls** | parallel для read-only, Sequential для мутаций | dispatch по ExecutionMode |

### 3.2 Python kernel: ограничения

- **Один kernel на сессию** — asyncio, не многопоточный. Подагенты получают
  СВОЙ kernel (отдельный процесс).
- **host_request — сериализован**: каждый запрос ждёт ответа. Нет batch API.
- **emit — fire-and-forget**: display события идут в TUI, без подтверждения.
- **bash handle — фон с уведомлением**: не блокирует kernel, но результат
  приходит через notification в следующем tool result.

### 3.3 Подагенты: изоляция и конфликты

- Каждый `rlm.spawn` → новый worker-процесс → **своя сессия, свои тулы,
  свой kernel**. ФС **общая** — два подагента могут редактировать один файл.
- **Cache key**: подагент шлёт `prompt_cache_key = parent` (не root) —
  та же проблема, что OpenCode #55243. Walk-to-root в бэклоге.
- **Компакция подагента**: не координируется с родителем — подагент может
  компактовать, пока родитель ждёт ответ.
- **Файловые конфликты**: нет блокировок. Единственная защита — whatever
  подагенты не редактируют один файл (не формализовано).

---

## 4. Решения без LLM: добавление RLM-специфики

| Решение | Где | Механизм |
|---|---|---|
| Keyless auth (opencode) | auth/manager.rs | KEYLESS_PROVIDERS константа |
| Arbiter совет | jev/tool_arbiter | таблица + ростер-фильтр |
| Graph rerank порядок | jev/graph_rerank | пороги + diversity в коде |
| Edit guards | pa-edit | unique-match, noop, seen-lines |
| Cache boundary | prompts/system_prompt | байтовая арифметика |
| Truncation caps | resources/skills | байтовые лимиты + char boundary |
| Trace honesty | pa-graph/trace | счётчик truncated |
| Changes unindexed | pa-graph/changes | счётчик unindexed_files |
| Admission breaker | pa-daemon | circuit per ProviderKey |
| Compaction trigger | compaction.rs | порог 85% + оценка |
| QA-оракулы | run_suite.py | state/output/logs (файлы, счётчики) |
| Faux/mock provider | pa-ai/faux | скрипт → ответы |
| **Kernel ready-check** | provisioner | prewarm → ready event (не sleep) |
| **RLM spawn validation** | rlm_host.rs | kwargs validation (unsupported keys → error) |
| **Skill loader safety** | skills/discovery | path policy, dependency tree skip |
| **Writethrough plan** | pa-lsp/writethrough | apply_text_edits plan после правки |

Все эти решения — код, не модель. LLM вызывается только для семантики
(jev probability, compaction summary, arbiter note enhancement).

---

## 5. Склейка нового и старого: расширенные кейсы (добавлены RLM-кейсы)

### Кейс 7: `skills/graph` (новое) × `native graph` (старое)
- **Точка склейки**: skill НЕ вызывает native tool (architectural limit). Skill
  возвращает готовый JSON call + `why` — модель его читает и выдаёт native call.
- **Каскад**: если native graph получит новое действие → skill route() надо
  дополнить → prompt_guards поймает дрейф action-имён.
- **Ограничение**: skill не может вызвать graph программатически — это
  осознанное разделение (Python = организация, Rust = исполнение).

### Кейс 8: `edit` skill (kernel) × `edit` native tool (Rust)
- **Точка склейки**: оба пишут файлы, но ядро не вызывает Rust-движок. Skill —
  своя реализация replace для Python-контекста (без pa-edit зависимостей).
- **Каскад**: у них разные гарантии — skill CRLF-preserving (мы добавили), но
  БЕЗ fuzzy matching (нет pa-edit fuzzy.rs). Если модель хочет fuzzy — она
  должна вызвать native edit, не skill.
- **Правило**: skill предназначен для Python-side targeted replace; native tool —
  для всего остального. SKILL.md это документирует.

### Кейс 9: подагент × родитель × ФС (конфликт правок)
- **Сейчас**: нет блокировок. Подагент может править файл, который правит родитель.
- **Что видно**: `rlm.spawn` передаёт prompt, model, thinking — но НЕ список
  файлов/директорий, которые подагент вправе менять.
- **Бэклог** (LayerFS-паттерн, 60-100ч): git worktree на пишущего подагента +
  merge-on-done. До этого: документировать правило «подагент не пишет в файлы
  родителя» в spawn prompt (недостаточно формально, но единственный механизм).

### Кейс 10: `cache boundary` × подагенты
- **Точка склейки**: каждый подагент строит СВОЙ промпт (своя сессия, свой
  prefix). `prompt_cache_key` — от parent (не root) — та же проблема #55243.
- **Решение**: walk-to-root при построении prompt_cache_key (2-4ч, бэклог).
  Подагент с тем же промптом → cache hit от родителя.

---

## 6. Метрики: добавление RLM-специфики

| Метрика | Формула | Источник | Цель | Семантика |
|---|---|---|---|---|
| **Kernel boot p95** | время prewarm→ready | provisioner | < 5s (уже ~4s) | старт не тормозит |
| **host_request latency p95** | round-trip kernel→Rust→kernel | protocol timing | < 100ms | python-скиллы отзывчивы |
| **Spawn depth distribution** | глубина подагентов | rlm.spawn events | median ≤ 2, max ≤ 4 | каскад под контролем |
| **Subagent tool mix** | tool_calls distribution per subagent | session JSONL | детект «сабагент не пользуется lsp» | семантика |
| **Cache key lineage** | % подагентов с root key (не parent) | после walk-to-root | 100% | кэш переиспользуется |
| **emit events per turn** | diff/attachment count | kernel emit | info-only | наблюдаемость |
| **Subagent file conflicts** | файлы, правленные ≥2 агентами | git diff или fs watcher | → 0 | формализовать после worktree |

---

## 7. Шпаргалка каскада: обновлённая для RLM

```
1. settings:  types.rs (тип+дефолт) → manager.rs getter → тест мержа
2. ростер:    engine.rs push + native_tool_names → СОРТИРОВКА сама
3. промпт:    native_tools_section (WHEN TO USE, honesty) → prompt_guards
4. кэш:       волатильное → в хвост; файлы → truncate_context_content;
              НЕ трогать префикс mid-session (OpenCode #14743)
5. потоки:    read-only → parallel; мутация → Sequential + lock;
              фон → tokio::spawn + spawn_blocking; single-flight если пишет
6. kernel:    если фича нужна python-скиллам:
              - файлы → skills/<name>/ (CRLF-preserving!)
              - host_request → новый тип + handler в rlm_host.rs
              - display → emit()
7. арbiter:   SuggestedTool-вариант + тест + native_tools_section
8. метрика:   что/где/знаменатель/цель/анти-метрика (см. MATRIX §6)
9. QA:        RT (надёжность), ST (семантика выбора), blackbox NC (сценарий)
10. подагент: не писать в файлы родителя (пока нет worktree);
              prompt_cache_key → walk-to-root
11. доки:     README + AGENTS.md + Synthesis
```
