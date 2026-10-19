# Синтез исследований для allspark — что делаем и почему

Дата: 2026-10-19. Источники: 12 исследований (Serena, oh-my-pi, Codex, Command Code,
Grok Build, codebase-memory-mcp, gitnexus, harness2, LayerFS, Mayoi, provider-caching docs,
black-box QA практики). Полные отчёты сохранены в `.research/learn_*.md`;
исходные материалы — в `.research/sources/`, дайджесты — в arsumbris.

## Подтверждённое обновление B.1

В локальном коммите `53c760290` завершён bounded advisory native LLM judge:
существующий `scripts/qa/run_suite.py` вызывает dev-only пример pa-ai один раз
после последней пробы задачи. Native OpenCode, без 9router. Ошибки/abstain
остаются `None`, не нулём и не scripted fallback. Известно загрязнённые секретом
данные не уходят в native bridge; strict deterministic gate не менялся.

Проверки: 23 Python-теста, red→green privacy/provenance, один живой smoke
HTTP200/816 токенов. Это не доказательство качества judge и не реализация PPC,
parallel eval runner или multi-model сравнения; последние остаются в бэклоге.
Подробно: `b1-llm-judge-report.md`, `b1-independent-review.md`.


## 1. Что уже перенесено (14 позиций, в коде, запушено)

Слой LSP-семантики (Serena): NamePathMatcher, 22 действия, diagnostics-diff с
привязкой к символу. Слой prompt-экономии (omp + provider docs): граница кэша на
проводе, дата в хвосте, детерминированный ростер. Слой надёжности (Mayoi/Codex):
6 fail-open фиксов, безопасный git, промпт-бомбы с лимитами. Слой качества
(собственный): graph авто-индекс с single-flight, black-box тест с negative
controls, 4 ST-задачи, opencode keyless. Каждый пункт — с регрессионным тестом.

## 2. Главная находка волны

**Модель выбирает тула по описаниям, а не по возможностям.** Наш graph умел всё
и не вызывался (132 python_repl против 1 graph), потому что его описание — меню
действий без единого триггера «когда». gitnexus решает это тремя приёмами:
WHEN TO USE голосом пользователя в первых 200 символах, якорь стоимости
(«answers in one call what would take 3-8 manual hops»), DAG связок AFTER THIS.
ST-002 поймала это вживую на реальной LLM — ответ верный, тула нет.

## 3. Что делаем дальше — три трека

### Трек A. Дешёвое и немедленное (эта неделя, ~3-5 дней)
1. **Описания graph/lsp по gitnexus-паттерну** (4-8ч; тексты готовы в
   learn_tool_descriptions). Верификация: unit-тест формы описания (WHEN TO USE +
   коридор длины) + ST-002 на реальной LLM (доля прогонов с >=1 вызовом graph).
2. **Tool-output потолок + tool-loop детектор** (3-5ч; Codex-инцидент: один
   результат на 948k токенов съел 49% лимита; 826k повторов вызова не ограничены).
   Форма — как у XERJ `max_tokens`: дедуп пересекающихся file:line, over-budget
   деградирует до locator-only, `_token_budget` accounting block, honest scope
   токенизатора в описании.
3. **Грейдер-файрвол real-model лейна** (2-3 дня; harness2-инцидент: агент сам
   скачал рубрики своей задачи — утечка ground truth).
4. **Measured gate для graph=auto** (1-2 дня; XERJ gate.rs: measured estimate
   больше порога → остановка ДО индексации с distinct exit code (не 1) и payload
   measured-опций: «мне нужно твоё решение» вместо тихой индексации; никогда не
   заявлять unmeasured speedup).
5. **Honesty sentences + truncation honesty в pa-graph** (0.5-1 день; XERJ
   BRAIN_HONESTY-паттерн: каждый тул говорит, чем он НЕ является; truncation
   reported-never-silent во всех выводах).
6. **Definition-first буст в graph.search** (8-16ч; XERJ def.rs: per-symbol docs,
   term name boost 12 / defs phrase boost 8 / code phrase boost 4; измерено 94%
   correct-file-at-rank-1 против 38% у body-search; плюс citable-line reorder).

### Трек A-bis. Протокол дельт + OpenCode-грабли (30-46ч, P1)
Анализ 5000 issues sst/opencode добавил в план:
- **Протокол «baseline + дельты»** (16-24ч) — ядро V2 OpenCode: все источники
  инструкций (AGENTS.md, skills, MCP, дата, env) — append-only дельты поверх
  frozen baseline, ordering most→least shared. У нас отсутствует — это главный
  отсутствующий примитив для стабильного префикса.
- **Root-derived cache key** для форков/сабагентов (2-4ч) — их #55243.
- **Маркер обрезки вывода с путём** (4-6ч) — их #11313 re-run-лупы.
- **Guardrails компакции** (10-14ч) — их #54993/#56350/#56784: 6+ багов одного
  класса (loops, непопадание в окно, clamped summary).
- **Поправка**: cap скиллов по количеству → байтовый бюджет + lazy (OpenCode
  не режет по количеству; жёсткий cap молча прячет функциональность).
- **Поправка**: LLMLingua НЕ для туловых выводов (портит идентификаторы кода);
  только структурная обрезка + файл + маркер.

### Трек B. Инфраструктура качества (2-4 недели)
4. **LLM-judge с контрактом None!=0** (1-2 нед) — без него «качество» не измерить.
5. **Parallel eval runner** (1 нед) — ST-задачи на реальных моделях прогоняются
   параллельно, дёшево, с resume.
6. **EditStore durable CAS-журнал** (40-60ч; LayerFS) — cross-session undo,
   audit, edit-blame; переживает рестарт воркера; ~КБ на правку.
7. **zstd-словарь для session JSONL** (24-40ч) — 13-25x сжатие истории.

### Трек C. Стратегическое (квартал)
8. **PPC поверх A.1-A.6** (harness2 F6): k кандидатов изменений харнесса,
   прогон через faux/mock, слепой selection, один урок в harness-memory. Их
   данные: даже 1 шаг стабильно положителен. Предусловие: трек B.
9. **git worktree + merge-on-done для подагентов** (60-100ч; LayerFS-паттерн)
   — когда мультиагентные правки начнут конфликтовать вживую.

## 4. Отсечённое (чтобы не возвращаться)

- Инкрементальная подсветка Mayoi: у нас нет цикла правка→кадр; parse_cache уже есть.
- Undo persistent tree: наш store.rs — версионный якорь, а не интерактивный undo.
- Docker-sandbox и FUSE LayerFS: ломают positioning «harness, а не платформа»;
  FUSE к тому же pre-1.0 lock до 21.10.
- 8-компонентная таксономия harness2 как догма: наш skills/memory/subagents/PTC
  выразительнее.
- Watch-inotify для graph: сначала freshness-check по git-signature на каждом
  запросе — дешевле и достаточно.

## 5. Метрики успеха

- ST-002 на реальной LLM: доля прогонов с >=1 вызовом `graph` — сейчас 0/1,
  цель >80% после трека A.1.
- Definition-first качество: после A.6 — correct-file-at-rank-1 на своём
  mini-benchmark (цель >80%; XERJ измерил 94% против 38% у body-search).
- hit rate промпт-кэша per-session в `sessions` команде — сейчас считается
  скриптом (90.5% общий, opencode 22%), цель: видимая метрика + alarm.
- tool-вызовы: медиана python_repl-per-edit < 2 (сейчас 4-9 в ST-задачах).

## 5a. ReactiveGraph — completeness envelope (добавлено)

Реактивграф (/root/reactivegraph) — третий независимый источник, решающий задачу
честности ответа графа (после cbm и XERJ). Перенять: `FileState{hash, state,
extractor_version}` + `Completeness{Complete/Partial/Stale}` + `next_step_hint()`
на каждый read-запрос search/trace/changes (8-16ч) — закрывает последний fail-open
класс «устаревший индекс неотличим от актуального» и естественно усиливает
graph=auto (фоновая сборка двигает Partial→Complete). Budget hard-fuse как тип
(4-8ч) — инструмент для tool-loop детектора. НЕ перенимать: RunFSM как общий
оркестратор, redb/RVF, WASM packs, witness log. Разбор: REACTIVEGRAPH-2026-10-19.md.

## 5a-bis. Компрессия системного промпта (добавлено)

Мы в зоне instruction saturation (150-200 инструкций; у нас 31+23 мандата + 126
буллетов). usage.md/opinionated.md сидят в середине префикса — худшая позиция
(lost in the middle). AGENTS.md-исследование (288 прогонов, p=1.00): контекст-файлы
не двигают pass rate, но режут cache-creation и wall-clock — резать безопасно.
План: C.1 абляция 4 руки (без неё резать — гадание) → C.2 cap скиллов 30 + описание
300 симв → C.3 кумулятивный бюджет context-файлов → C.4 сигнатуры тулов из core.md
в схемы (−2.7k tok, 72% core) → C.5 usage.md правила→суждение → C.6 project-context
только готчи. Анти-паттерн: экономить prefix ради денег — он 0.1× цены.
Разбор: COMPRESSION-2026-10-19.md.

## 5a-ter. OpenCode issues — грабли компакции и кэша (добавлено)

Сканирование 5000 issue titles sst/opencode. Топ-классы:
1. **Compaction loops** (6+ issues): fixed part (system+tools) > window → бесконечный
   цикл (261 цикл, $20 у одного пользователя); bookkeeping не персистится → loop
   после aborted stream; provider клампит summary → retry loop. Для нас: persisted
   state, refusal при fixed > window (не loop).
2. **Cache busting**: committed revert сбрасывает instruction baseline → 99% hit
   становится 0% (#54408); fork-of-fork шлёт parent cache key вместо root (#55243,
   PR с walk-to-root приложен). Для нас: проверить revert/fork не сбрасывают ли.
3. **Reuse-meter** (#56422): mock-provider, который меряет reuse → превращает
   «маркер стоит» в «кэш экономит N%». Их замер: default = 91%, drop tail bp = +66%.
4. Их system prompt = **756 байт** — не копировать (мы даём больше возможностей),
   но перенять принцип «инструкции в tool description + условия вместо правил».
5. Skill watcher — 8 багов (self-loop, 100% CPU); мы без watcher — правильно.
   Skill output cap 50KB у них — наш 256 KiB щедрее.

## 5a-quater. Arsumbris vault аудит (добавлено)

Аудит ~200 заметок нашего же vault: 60 находок. Ключевые:
- **JEV middle-зона 0.55-0.85 систематически лжёт** — наш judge должен конфликтовать
  None-контрактом (не 0), и пороги не фиксировать на 0.5
- **Shadow-порог делегирования** (c(s) ≥ q_L - ΔC/V = -14% цены, 0 потерь) —
  применимо к tool_arbiter: порог должен быть динамическим, не фикс 0.5
- **Acceptance-критерий refine**: должен видеть инфраструктурный вклад + вердикты
  helped/hurt/confounded (не score-only)
- **Аннеалируемый бюджет правок** для continual harness (RRSI: без бюджета OOD-прирост
  схлопывается)
- **Контроль в код вместо контекста** бьёт on-demand скиллы (100% vs 53%)

Разбор: learn_arsumbris_jev_harness.md.

## 6. Полный анализ применимости

Этот документ — краткий. Полный разбор по каждому исследованию (судьба каждой
находки: 42 перенесено / 47 в бэклоге / 34 отсечено, 5 поперечных
закономерностей) — в `APPLICABILITY-2026-10-19.md` рядом.

## 7. Ключевой принцип на будущее

Каждое исследование закрывается тремя артефактами: отчёт (в `.research/` репозитория), дайджест
(в arsumbris), и либо код с регрессионным тестом, либо строка в этом бэклоге
с оценкой. Исследование без четвёртого — развлечение.
