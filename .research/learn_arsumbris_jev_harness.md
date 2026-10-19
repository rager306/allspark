# Аудит arsumbris vault: JEV (typed calibrated decisions) и harness-паттерны

Дата: 2026-10-19. Метод: прямой grep + чтение 548 md-файлов `/root/vault` (knowledge 377, inbox 128, self 3, ops 24) + graph-анализ wikilinks. Read-only, ничего в vault не изменено. Каждая находка — с путём к заметке.

---

## 1. JEV: паттерны, ограничения, негативные результаты (~90 заметок упоминают jev; 59 с темой [[jev]])

### 1.1 Что такое Jev в корпусе
Семейство decision-моделей («System One»): один forward pass, три типизированных вопроса — `noul` (да/нет с калиброванной вероятностью), `choice` (выбор из ≤255 опций), `score` (ожидаемый уровень) — ноль генерации, $0 за output-токены. Протокол де-факто стандарта: `POST /v1/systemone` (TypeSafe Jev, Solar Decide, Nimble, Drex, Agr, Kev, Ollaya-каталог).

### 1.2 Паттерны применения
- **Трёхуровневый гейт: правила → Jev → человек** — `knowledge/jev-deterministic-first-gating-pattern.md`. Детерминированные правила съедают ясные случаи, Jev — серую зону, человек — low-confidence/high-severity band. Экономика cost asymmetry: правило ~0$, Jev — доли цента и 70–500ms, человек — самое дорогое. Обязательные свойства: fail-closed, shadow mode перед включением, аудит-лог.
- **Jev выбирает типизированное действие, LLM — только текст/план** — `knowledge/jev-agent-action-selection-pattern.md`. Browser/робот-агенты: 30–50% экономии токенов. Ограничение из полевых данных: «Jev — выборчик, не планировщик»; без правил collision-avoidance в коде поведение хаотично.
- **Decision-модель делит контроль с LLM-агентом** — `inbox/jevagentharness-decision-models-share-control-with-llms.md` (arXiv 2610.21412). 8 decision points × 5 integration modes (Replace/Assist/Guard/**Cascade**/Trigger). Ключевое: порог уверенности `c(s) ≥ q_L − ΔC/V`, фиттится на **shadow-траекториях самого агента**: −14.2% цены при −0.0 п. успеха. Каскад coding-агентов почти не экономит (только route: +7–10.5%) — free-text шаги decision-модель не отличает.
- **Фильтрация вместо чтения, вопрос = политика** — `inbox/the-signal-engine-filtering-beats-reading-and-the-question-is-the-policy.md`: код-фильтры → Jev ($0.03/864 постов) → LLM читает только выживших.
- **Jev-компакция transcript бьёт LLM-summary** — `knowledge/jev-compaction-beats-summary-on-hermes-transcripts.md` (own experiment): 83.3% vs 76.7% recall при 17% контекста, $0.009 на 276 решений. Причина: recall деградирует от context-rot пересказа, а не от удаления «шума».
- **Компактация как сохранение рубля**: собственный judge-стек с shadow-логом и замером калибровки — `inbox/judge-calibration-from-verdict-matching-to-measured-reliability.md` (Brier 0.181, ECE 0.036; middle-зона 0.55–0.85 систематически переоценена).

### 1.3 Рынок/модели (срез 2026-09…10)
- `inbox/jev-1-14-...-262k-context-and-vision.md` — инкумбент ответил: 262K контекст, vision, $0.042/M in.
- `inbox/nimble-v4-...md` — заявка на корону (9B 63.70; 27B 68.46, самозамер).
- `inbox/drex-2-0-...md` — MoE 35B-A3B 61.25 Decision Index; «никто не доминирует всюду» (Jev держит reasoning-бенчи).
- `inbox/openai-decisions-api-...md` — вход OpenAI валидирует категорию, но confidence без опубликованной калибровки ≠ калиброванная вероятность.
- `inbox/agr-...`, `inbox/kev-...`, `inbox/jeff-...`, `inbox/ollaya-...`, `inbox/the-jev-leaderboard-matures-into-an-institution.md` — коммодитизация: открытые клоны догоняют за месяцы; wiring-контракт отделился от вендора.
- `inbox/an-agent-loop-beats-jev-at-its-own-game-...md` — RSI-петля построила decision-модель, обошедшую вендора на публичном борде (65.64 vs 57.96), с зарегистрированными предсказаниями и публикацией всех 496 экспериментов.

### 1.4 Ограничения и негативные результаты (важнее паттернов)
- **Панель ≠ поле**: Jeff 2B на бенч-панели 82.0 vs Jev 83.0, но у практика в поле 70% vs 94% — «один балл панели = 24 в поле» (`inbox/jeff-is-the-one-gpu-commodity-floor-...md`). Тот же урок в `inbox/legal-agents-...`: judge coverage 84–92% на своих данных.
- **Порог без фиттинга хуже отсутствия делегирования**: фикс 0.5 → −21.5 п. успеха; фиттнутый shadow-порог → 0 (`inbox/jevagentharness-...md`). Честный негатив: Flash-Lite + порог = все выборы остались LLM, экономия ноль.
- **Не делегировать необратимые действия**; coding-агентам decision-модель даёт мало (`inbox/jevagentharness-...md`).
- **Средняя зона уверенности лжёт**: у собственного judge 0.6→observed 0.18, 0.8→0.00 (`inbox/judge-calibration-...md`).
- **Опцион-порядок и train-подгон**: порядок опций меняет ответ (Agr/Kev); учить на train-split бенча = +0.12 и потеря general knowledge — эксперимент отвергнут петлёй (`inbox/an-agent-loop-...`).
- **Не safety-фильтр**: все клон-карточки прямо запрещают использовать как safety-гейт без проверки на своих данных.

---

## 2. Harness design: что работает, что нет (~33 заметки с «harness»)

### Что работает
- **Повторяющийся контроль — в код, не в контекст** — `inbox/growing-harness-...` (−76–92% LLM-вызовов, −74–99% стоимости; на 4B держит 45% где tool-calling падает до 6.7%).
- **Один персистентный REPL вместо фрагментов тулов** — `inbox/prime-agent-replaces-tool-fragments-...`: prompt-as-variable, один инструмент ipython, continual harness с 5 видами записей и «маленькие evidence-backed правки».
- **Пассивный контекст бьёт on-demand скиллы, когда решение о вызове оставлено агенту** — `knowledge/passive-context-agents-md-achieves-100-percent-...` (100% vs 53%) и `knowledge/agent-skills-go-unused-in-56-percent-...` (56% случаев скилл вообще не вызван). Примирение с обратным результатом SkillsBench — в этих же заметках: интеграция в workflow решает.
- **Разделение имплементационных тестов и независимой оценки; гейты на верифицируемых выходах, свобода workflow** — `inbox/harness-of-harness-...` (+52.25% средний прирост).
- **Контрастный метод без верификаторов** — `inbox/harness2-contrastive-propose-probe-compose-...`: behavioral contrast вместо score; Experience Reflector пишет verdict `helped|hurt|no effect|confounded` (confounded честнее изобретённой атрибуции) и максимум ОДИН переносимый урок.
- **Модульная декомпозиция харнесса** — `inbox/modularrsi-...`: 5 модулей (Agent Loop, Tool Use, Observation Mgmt, Context Mgmt, Completion Detection), эволюция в ограниченном scope.

### Что НЕ работает
- **Score-only acceptance** отвергает инфраструктурные правки (чинил логирование → скор не вырос → откат), хотя именно они дают улики для следующего ремонта — `inbox/recursive-self-improvement-survey-...` §3.6, ключевой разрыв всего поля.
- **Оптимизация под одиночный провал** = model-specific accommodation; персистентная правка оправдана только **повторяющимися между задачами** провалами — `inbox/ecdysis-...`.
- **Переобучение эволюции**: in-distribution приросты исчезают OOD — `inbox/rrsi-...` (лечится: аннеалируемый бюджет правок, критик benchmark-специфичного, прунер); копление локально-полезных правок в избыточные инструкции — `inbox/skillevoreg-...` (dropout блоков, complexity cap, causal counterexample validation).
- **Скилл без управления rot**: кэш рассуждения −63% стоимости, но тот же цикл компаундирует дрейф стоимости, rot и trust surface (кейс $47 сюрприз-счёта) — `knowledge/skills-are-a-reasoning-cache-...`.
- **Compaction loops — крупнейший класс полевых багов харнеса**: fixed part > window → 261 цикл и $20; cache-busting от revert/fork-lineage; skill watcher = 8 багов — `inbox/allspark-opencode-issues-compaction-loops-cache-busting-756-byte-prompt.md`.
- **Модель выбирает тул по описанию, а не по возможности**: graph умел всё и не вызывался (132 python_repl против 1 graph) — `inbox/allspark-synthesis-12-research-sources-three-tracks.md`.

---

## 3. RSI / self-modification (~82 заметки; ядро — 18-хабовая заметка-обзор)

- **Каркас всего поля** — `inbox/recursive-self-improvement-survey-...`: U = R∘F∘P, 9 измерений (артефакт/организация/расписание/фидбек); DL-механизмы переносятся (SkillAdam=Adam-моменты для скиллов `inbox/skilladam-...`; TextGrad; RRSI=L0-регуляризация).
- **Архив вместо цепи** — `inbox/darwin-godel-machine-...`: сэмплирование родителей из архива валидированных правок (SWE-bench 20→50%); git-история = готовый архив, но её надо сэмплировать, не только хранить.
- **Meta-productivity вместо benchmark score** — `inbox/huxley-godel-machine-...`; сравнение трейсов линий — `inbox/mendel-godel-machine-...`.
- **AIDE²**: 8-дневный самособственный ран; найденные улучшения = поисковая политика и **компрессия растущего контекста**; reward hacking упал 55→32% как побочный продукт лучшей инфраструктуры — `inbox/aide2-...`.
- **SEAL** — эталон параметрической ветки (self-edits → SFT, награда = downstream) — `inbox/seal-...`.
- **Harness-Zero**: харнесс-поведение дистиллируется в веса, потом харнесс снимается (23.3→44.3%, выше чем 41.7% с харнессом) — `inbox/harness-zero-...`. Обратная сторона вечно живущего харнесса.
- **COBRA**: бюджет оценок тратить как бандит (−55–58% стоимости) — `inbox/cobra-...`; **GDPevo**: наивный skill-creator бьёт навороченных; оракул-потолок 91.6% — все агенты далеко ниже — `inbox/gdpevo-...`.
- **Перенос опыта**: XferAgent — сливай опыт только с маржой, иначе новый узел; route по чужим исходам — `inbox/xferagent-...`; **CHIME** — attribute-before-memorize, раздельные банки планов и исполнения — `inbox/chime-...`; **TTSE** — факты и процедуры эволюционируют на разных колеях — `inbox/ttse-...`; **EvoMemBench** — long-context бьёт большинство memory-методов, ни одна форма памяти не выигрывает везде — `inbox/evomembench-...`.
- Собственные живые эксперименты: frontier-оркестратор фаза 1 (judge gate p≥0.30, цикл $0.004) — `inbox/frontier-orchestrator-phase-one-closed-...`; пучковый лапласиан как детектор противоречий субагентов (да как диагностика, нет как консенсус) — `inbox/sheaf-laplacian-diagnostics-...`.

## 4. Prompt compression / context engineering (~20 заметок)

- `inbox/allspark-prompt-compression-methods-and-ablation-plan.md` — главный документ: prefix=0.1x цены (кэш), хвост=1.0x → компрессия про saturation, не про деньги; instruction saturation 150–200 инструкций = зона пика; lost-in-the-middle бьёт файлы в середине префикса; AGENTS.md не двигает pass rate (p=1.00) но режет cache-creation; 48×512 симв скиллов — выше нормы Claude Code (≤30 total); tool-сигнатуры из промпта в схемы = −25% prefix. Анти-паттерны: резать 80% без абляции, LongLLMLingua на системный промпт, верить «40x fewer tokens».
- `inbox/allspark-opencode-issues-...` — 756-байтный системный промпт OpenCode работает (16/16): принцип «инструкции в tool description + условные правила»; persisted compaction state, refusal при fixed>window.
- `knowledge/context-window-stuffing-...` — retrieved memories ≤20–30% окна, relevance thresholding вместо top-k.
- `knowledge/code-actions-outperform-text-json-...` — код как action space: +20% success, −30% turns (обоснование REPL-архитектуры prime-agent).
- `knowledge/sqlite-fts5-handled-a-3799-page-manual-...` — не всякому контексту нужен vector db.
- `inbox/evomembench-...` (см. выше) + `knowledge/a-3am-dreaming-job-compiles-the-day-into-tomorrows-context-...` — контекст следующего дня собирается ночным cron.

---

## 5. Связи (wikilinks): где темы пересекаются

Граф: 505 текущих заметок, хабы по входящим ссылкам: memory-systems (80), zeroclaw-decisions (75), index (68), agentic-architecture (65), **jev (61)**, rust-ai-systems (39), tool-use (38), RSI-обзор (18), self-improvement (13/18).

Ключевые пересечения (co-occurrence тем во frontmatter):
- **memory-systems × tool-use** (11) и **agentic-architecture × tool-use** (11) — контекстная инженерия лежит на стыке памяти и тулов.
- **agent-security × tool-use** (7) — гейты/калибровка живут там же (jev-deterministic-first-gating).
- **hermes-user-stories × self-improvement** (5) и **memory-systems × self-improvement** (5) — полевые истории про экономику self-improvement.
- Jev-кластер сшивает три контура: security-гейты (jev-deterministic-first, judge-calibration) ↔ harness-делегирование (jevagentharness) ↔ рынок/коммодитизация (kev/agr/drex/nimble/leaderboard).
- RSI-обзор — хаб-заметка: на неё замкнуты growing-harness, ecdysis, ModularRSI, RRSI, SkillAdam, SkillEvoReg, SEAL, DGM, AIDE², HoH, Harness-Zero, SoL-Pi, JAZ, SEEK, COBRA, TTSE, CHIME.

## 6. Пробелы (упомянуто, но не раскрыто / деградация связности)

1. **Статус-гигиена**: 505/505 заметок status=current, 0 stale/superseded — при freshness:volatile у 117 заметок про модели/борды, которые уже устарели (борд менялся трижды за волну). Механизм decay задуман (vault-freshness-system) — `inbox/vault-freshness-system-classifies-notes-by-decay-speed.md`, но не применяется к status.
2. **Отдельная заметка про negative results Jev-делегирования в coding-агентах** отсутствует — самый переносимый негатив разбросан по 2 файлам (jevagentharness + jeff).
3. **Calibration-методология** есть для judge (`judge-calibration-...`), но нет заметки про перенос на harness-пороги (q_L − ΔC/V) и наShadow-режимы скиллов; темы [[judge-stack-shadow-mode]] и [[frontier-orchestrator]] используются как topics, но отдельных узлов-заметок нет (висячие темы).
4. **Cost-engineering как тема** не имеет MOC: цифры рассыпаны по 10+ заметкам ($0.004 цикл, −63% скилл-кэш, prefix 0.1x, $47 сюрприз), нет сводной экономики.
5. `self/goals.md` отстаёт от фактической работы: backlog про Rust-паттерны ZeroClaw, тогда как 110 из 128 inbox-заметок за октябрь — про Jev/harness/RSI/allspark.

---

## ТОП-5 идей для allspark (форк /root/prime-agent)

1. **Cascade с фиттнутым shadow-порогом на pa-jev** (`inbox/jevagentharness-...`): decision-child (SystemOne) отвечает первым на tool-choice/аргументы; порог `c(s) ≥ q_L − ΔC/V` фиттится на shadow-траекториях самих сессий. Ожидание по бумаге: −14% цены при −0.0 п. качества. Не делегировать необратимые и free-text шаги.
2. **Acceptance-критерий рефайнмента должен видеть инфраструктурный вклад** (RSI-обзор §3.6 + Ecdysis): сегодня `refine` = «маленькие evidence-backed правки»; добавить вердикт-атрибуцию `helped|hurt|no effect|confounded` в refinement-историю и порог повторяемости (провал в ≥2 разных контекстах) прежде, чем урок становится prompt-note/скиллом.
3. **Контрастный PPC-цикл как W5** (`inbox/harness2-...` + `inbox/allspark-synthesis-...` трек C): reference execution → кандидаты (invariant+domain) → behavioral probe → compose; Experience Reflector пишет максимум один переносимый урок. Наивный creator уже достаточен (GDPevo) — не переусложнять первую версию.
4. **Бюджет правок + прунер для continual harness** (`inbox/rrsi-...`, `inbox/skilladam-...`, `inbox/skillevoreg-...`): аннеалируемый лимит правок за раунд, volatility-driven (большой разброс приростов → меньше править), периодический прунер записей, не дающих прироста; complexity cap на рост скиллов.
5. **Компрессия промпта по абляционному плану C.1–C.6** (`inbox/allspark-prompt-compression-...`): сначала 4-рукая абляция (usage.md — главный кандидат на harmful), cap скиллов 30×300 симв, кумулятивный бюджет context-файлов, потом сигнатуры в tool-схемы (−25% prefix). Экономить хвост (1.0x), не prefix (0.1x). Плюс persisted compaction state и refusal при fixed>window из opencode-разбора.

## ТОП-3 пробела для пополнения vault

1. **Заметка «negative results Jev-delegation» + замер на allspark**: свести jevagentharness/jeff/judge-calibration в один узел с чек-листом «когда НЕ подключать decision-модель» (coding free-text, необратимые, middle-зона 0.55–0.85) — и own-экспериментом на реальных траекториях allspark.
2. **MOC «cost-engineering агентных петель»**: свести цифры экономики ($0.004 цикл оркестратора, −63% скилл-кэш, prefix 0.1x/хвост 1.0x, compaction-loop $20, Jev-компакция $0.009) в одну заметку с метриками и анти-паттернами.
3. **Статус-обвалка volatility-заметок**: применить freshness/decay к 117 volatile-заметкам про модели/борды (помечать status: stale при выходе новой волны — прецедент jev-1-14, обнулившей корону Nimble/Drex) и зафиксировать правило: борд-цифры живут ≤1 волну без обновления.
