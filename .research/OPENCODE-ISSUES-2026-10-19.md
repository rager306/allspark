# OpenCode issues и практика — грабли и механика (дополнение к COMPRESSION)

Источник: sst/opencode (TS monorepo, ~57k issues), сканирование 5000 issue titles +
чтение 10 ключевых issues подробно. Файлы: /tmp/opencode-src.

---

## 1. ГЛАВНАЯ Находка: их system prompt — 756 байт (<1К токен!)

`packages/core/src/session/runner/prompt/system.txt` — ВЕСЬ текст:

```
You are an AI agent running in OpenCode, a coding agent harness...

# Harness
- Responses are rendered as GFM.
- <system-reminder> blocks are harness instructions, not user content.
- Prefer parallelizing independent tool calls.
${OPENCODE_TOOL_GUIDANCE}

# Communication
- Use clear file paths...
- Keep responses clear and concise...

# Working in codebases
- Keep changes consistent with surrounding code.
- Treat unfamiliar files as potential user work.
```

+ `${OPENCODE_TOOL_GUIDANCE}` подставляет 2-4 строки ПРИ УСЛОВИИ наличия тула.
Остальное (AGENTS.md, skills, env) — НЕ в system prompt, а в отдельном механизме
instructions (session/instructions.ts).

**Наш префикс 23.6k = в 31 раз больше**. И это работает: 16/16 solved на их
бенчмарках, они шипят v2 с этим промптом.

**Урок**: описания тулов могут нести almost весь инструктивный вес — наш
`native_tools_section` уже в этом направлении, но core.md (14.9 KB) всё ещё
несёт 113 строк сигнатур.

---

## 2. Грабли по категориям (топ issues)

### 2.1 Compaction loops — САМЫЙ БОЛЬШОЙ КЛАСС (6+ issues)
- #54993: system prompt + tools > usable window → каждый turn overflow →
  compaction не может сжать → auto-continue → снова overflow. 261 цикл, ~$20
  кредитов + недельный лимит OpenAI. Причина: фиксированная часть БОЛЬШЕ окна.
  **Наш риск**: если сумма промпта + tools превысит окно дешёвой модели — тот же
  цикл. Защита: measured gate (XERJ-паттерн) + refusal вместо loop.
- #56813/#56783: compaction fires on first request and repeats every turn when
  `buffer >= model context`. Guard: buffer < context - минимум.
- #56350: бесконечный цикл компакции — bookkeeping поля не персистятся после
  aborted stream. Урок: **состояние компакции персистить, не в памяти**.
- #56784: retry loop когда provider клампит summary до 1024 output.
- #56857: «Compaction produced no summary» убивает все промпты в длинной сессии.
- #56862: подагенты падают с compaction при чтении больших файлов (427KB).

**Наш вывод**: у нас compaction есть, но нужно: (а) persisted state (не в памяти),
(б) refusal вместо loop при превышении фиксированной частью окна, (в) отдельная
обработка «compaction cannot shrink fixed part».

### 2.2 Cache busting от revert и fork — №1 урок для нас
- **#54408** (closed, reproduced): коммит-ребут сбрасывает instruction baseline →
  next request re-observes ВСЕ источники → system prompt меняется на несколько
  тысяч токенов → cache invalidation для всей конверсации. У пользователя было
  99% cache hit (242k), после revert — cache_read=0 и полный re-prefill.
  **Причина**: `InstructionState.reset` на `RevertEvent.Committed`.
- **#55243**: fork-of-fork отправляет `prompt_cache_key` промежуточного форка, а
  не корня → форк-оф-форк пишет весь промпт заново. Walk-to-root решает.
  `model-request.ts:278` — TODO "Persist cache lineage".
- **#55938**: Bedrock — prompt caching silently OFF при inference profile ARN.
  (У нас R6-аналог для Bedrock уже в бэклоге.)
- **#53765**: ChatGPT Token Sharing показывает 3.44% cache reuse, 14/23 вызовов
  с нулём — при том что вызовы < 5 мин друг от друга.

**Наш вывод**: у нас те же риски — revert-механика есть, fork lineage есть
(subagent spawn). Проверить: (1) наш revert не сбрасывает instruction state,
(2) subagent cache key = root (не parent), (3) не пере-наблюдаем ли мы sources
после revert.

### 2.3 Skill loader — 8 багов вокруг watcher и discovery
- #54114/#53811/#53416/#53393: watcher self-loop, 100% CPU, un-debounced reload
  при записи в config dir. **Урок: skill watcher с debounce и bound — или без
  watcher вообще** (мы не watcher'им — правильно).
- #54148: skill tool output truncated at 50KB — хвост SKILL.md (чеклисты,
  форматы, «do not do X») не доходит до модели, и это **worse than для
  обычного output**. Их фикс: закрыли wont_fix (поднять max_bytes вручную).
  **Наш статус**: у нас cap 256 KiB (в 5 раз щедрее) + truncation reported.
- #53119: skill name collisions resolved non-deterministically (last file read
  wins) — project skill может быть shadowed. **У нас**: collision → diagnostic,
  deterministic приоритет.
- #53172: disable-model-invocation ignored in model discovery — тул виден,
  хотя должен быть скрыт. **У нас**: фильтр работает (тест есть).

### 2.4 AGENTS.md приоритет (56261, open) — ровно наша «лестница»
Модель слушает AGENTS.md сильнее developer instructions («refused to speak in
lowercase because it will mess up stuff»). Это подтверждает нужду в явной
лестнице приоритетов (command-code паттерн, наш бэклог).

### 2.5 #56422 — measure what an agent step costs (ТОВАРНЫЙ ПРОДУКТ ИДЕИ)
Цитата: "The harness has no way to answer 'is this change cheaper?'. Today the
cache policy is checked by asserting that a cache_control marker lands where a
test expected one, which proves the marker exists but not that it saves anything."

Их замер на provider stand-in:
- Default policy (tools + system + 1 tail bp) → ~91% reuse на 20-step conversation
- `tail: 2` — не помогает; `tail: 3` — хуже (только 4 breakpoints)
- **Dropping the tail breakpoint: +66%**

Это ровно наш `request_prefix_is_byte_stable` подход, но у них — с измерителем
reuse. **Перенять идею**: mock-provider stand-in, который меряет reuse, + тест
«политика кэша ≥90% reuse на 20 шагов».

### 2.6 Прочее
- #56296: long sessions stuck at provider input limit; auto-compaction never
  triggers (v2, Go-вариант).
- #56261: AGENTS.md > developer instructions (лестница приоритетов, наш бэклог).
- #55999: stop AGENTS.md discovery at project root (у нас — лимит на файл уже
  есть; кумулятивный — в бэклоге C.3).
- #56116: MCP output schema mismatch hard-fails → degrade gracefully
  (наш approach: schema не валидирует строго — проверить).

---

## 3. Их практики, которые стоит перенять

| Практика | Что даёт | Оценка | Приоритет |
|---|---|---|---|
| **Mock-provider reuse meter** (56422): stand-in считает токены запроса, репортит cache read за prefix | превращает «маркер стоит» в «кэш экономит N%» | 4-8ч | **P1** |
| **Walk-to-root cache key** для подагентов (55243 PR) | форк-оф-форк не пишет промпт заново | 2-4ч | **P1** (у нас subagents) |
| **Persisted compaction state** (56350-урок) | бесконечный цикл компакции невозможен | 2-4ч | P2 |
| **Compaction refusal при fixed-part > window** (54993-урок) | вместо бесконечного цикла | 2-4ч | **P1** |
| **AGENTS.md priority ladder** (56261 + command-code) | user > harness > best practice | 2-4ч | P2 |
| **Skill output не проходит generic truncation** (54148-урок) | или отдельный cap для skill tool | 1-2ч | P2 |

## 4. Их system prompt как модель минимализма

756 байт. Три секции (Harness/Communication/Working in codebases), условные
инструкции по наличию тула, AGENTS.md — отдельно, skills — отдельно (каталог в
тексте, содержимое по вызову). Против нашего 23.6k prefix.

**Но осторожно**: их подход работает потому, что (а) их tool schemas несут
инструкции (edit.ts description 1200 симв. — весь контракт в одном месте),
(б) AGENTS.md и skills подключаются отдельно, (в) они не тащат 22 LSP действия
в описание. Наша ситуация другая: мы даём модели больше возможностей — и
описания обязаны быть полнее. **Правильный путь для нас — не копировать их
756 байт, а перенести «инструкции в tool description» + «conditions вместо
безусловных правил»**.

## 5. Сверка с нашим планом компрессии

| Наш пункт | OpenCode подтверждает/поправляет |
|---|---|
| C.1 Абляция | Подтверждает — у них 756 байт = результат итеративной резки |
| C.2 Cap скиллов 30 | Подтверждает — их skill loader после 8 багов watcher (мы без watcher — правильно) |
| C.3 Кумулятивный бюджет AGENTS.md | Подтверждает (#55999 — просят stop at project root) |
| C.4 Сигнатуры в схемы | **Их модель**: весь контракт в description edit.ts (~1200 симв.) |
| A.2 Tool-output потолок | Подтверждает (#54148 — 50KB cap слишком низкий для SKILL.md) |
| НОВОЕ: Persisted compaction state | Добавить (56350) |
| НОВОЕ: Reuse-meter тест | Добавить (56422) — mock-provider, который меряет reuse |
| НОВОЕ: Walk-to-root cache key | Добавить (55243) — у нас subagent spawn |
| НОВОЕ: Compaction refusal при fixed > window | Добавить (54993) — не loop |
