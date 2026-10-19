# graph=auto: синтез дизайна по четырём источникам

Дата: 2026-10-19. Задача: строить граф `pa-graph` автоматически в фоне при старте сессии,
чтобы к первому запросу он был готов; не мешать пользовательским запросам; обнаруживать
изменения файлов. Источники: установленный codebase-memory-mcp (его исходники = DeusData-репо,
бинарь 299MB C++, локальных исходников нет — сверка по репо), DeusData/codebase-memory-mcp,
win4r/codebase-memory-mcp-pro, /root/reactivegraph + наш код.

---

## Часть 1. РАЗБОР ИСТОЧНИКОВ

### 1.1 DeusData/codebase-memory-mcp (`/tmp/cbm-deus`, C++)

Это эволюционировавшая версия того же продукта, что стоит у нас бинарем
(`/root/.local/bin/codebase-memory-mcp`); win4r-«pro» — её более ранняя, простая редакция.

**Триггер индексации — трёхступенчатый (lazy + eager-tail):**

1. *Хвост первого запроса.* Успешный MCP `initialize` только ставит флаг:
   `src/daemon/application.c:3117-3124` (`session->pending_background_initialize = true`).
   После диспетчеризации ответа хвост запроса (`application_request_dispatch`, application.c:3366-3385)
   при `!cancelled && status==OK` вызывает `application_background_initialize()` —
   автоматическая работа НИКОГДА не задерживает ответ клиенту.
2. *Политика допуска.* `application_background_initialize_impl` (application.c:2360-2446):
   та же workspace-граница, что у явного `index_repository` (автоматика не шире ручной работы),
   bounded-подсчёт файлов с таймаутом 5 c (`cbm_discover_count_bounded`, mcp.c:20754-20783),
   скип `too_many_files` (лимит `auto_index_limit`, дефолт 50 000, `src/mcp/mcp_internal.h:70`;
   ресурсные потолки `index_max_files`=10M / `index_max_source_mb` в
   `src/foundation/index_policy.h:9-12`).
3. *Однократность и retry.* Сессия хранит `auto_index_evaluated` (оценили один раз),
   `auto_index_subscribed`, `auto_index_retry_pending` (application.c:125-132). Занятый
   worker → `retry_pending=true`, ре-арм при следующей возможности координатора
   (`application_auto_index_retry_pending_locked`, application.c:1716-1755) — изменение
   не теряется, но и не крутится горячим циклом.

**Где живёт работа:** отдельный поток → а внутри предпочтительно **supervised worker subprocess**
(`autoindex_thread`, `src/mcp/mcp.c:20703-20752`): причина #832 — долгоживущий серверный поток
индексации «ratchet-ит» RSS (аллокатор не возвращает страницы); дочерний процесс, который
индексирует и выходит, отдаёт 100% памяти ОС каждый цикл. Перед запуском — глобальный
`cbm_pipeline_lock()` (одновременных пайплайнов нет), после — освобождение кэшей.

**Не мешать запросам:** индекс пишется в SQLite; читатели идут мимо пайплайна. Ответ на запрос
при отсутствии индекса — структурированная ошибка, а не ожидание: `verify_project_indexed`
(pro/mcp.c:1072-1093, то же в deus) возвращает `"project not indexed — run index_repository first"`.
То есть **partial results не раздают**: пустой/частичный ответ опаснее честной ошибки.

**File watching — НЕ inotify.** `src/watcher/watcher.h:1-10`, `watcher.c:1-25`:
**git-polling** — HEAD-хэш + подпись «грязного» состояния (porcelain + per-file size/mtime,
#937). Константы: `POLL_BASE_MS=5000`, `POLL_MAX_MS=60000` (watcher.c:143-145), адаптивный
интервал `5s + 1s за каждые 500 файлов` (`cbm_watcher_poll_interval_ms`, watcher.c:208-210).
Non-git корни не поллятся, пока явно не включён `watch_non_git` (#1948) — тогда подпись
дерева: обход теми же ignore-правилами, fold по (relpath, size, mtime).
Ключевые инварианты watcher:
- **Базлайн коммитится только после успешного reindex** (index_fn вернул 0); busy-skip/ошибка
  оставляют pending → событие retried, never lost (#937; watcher.h:16-24).
- **Backoff по числу жёстких отказов**: задержка удваивается с потолком
  `INDEX_FAIL_SHIFT_MAX=6` (watcher.c:151), сброс на успехе — «never lost ≠ retried forever».
- **Prune несуществующих корней** только по ENOENT/ENOTDIR (`cbm_watcher_root_missing_errno`);
  EACCES/EIO не считаются — кэш-DB содержит пользовательские данные, её нельзя выкашивать
  по случайной ошибке (#286). Удаление координируется с daemon-подписками
  (`cbm_watcher_set_project_mutation_guard`, begin/end non-blocking).
- **Статус наблюдаем** (#2167): `cbm_watcher_project_info` отдаёт strategy
  (PENDING/GIT/NONE/TREE), текущий интервал, время последнего скана — поля публикуются атомиками.

**Конфиг:** `auto_index` (дефолт **false**!), `auto_index_limit` (50000), `auto_watch`
(дефолт true, `CBM_CONFIG_AUTO_WATCH`, application.c:570-571), `watch_non_git` (дефолт false).
Важно: watch регистрируется **только когда DB уже существует** (application.c:592-612:
`!enabled || !db_exists` → снять watch) и только для проекта собственной MCP-сессии,
пока сессия открыта; watch-подписки считаются (subscribers), физический watch — один.

**Тестируемость фона:** атомик-счётчики и инъекции отказов как test seams —
`g_application_background_initializes_for_test`, `fail_next_job_thread_start_for_test`,
`hold_job_before_start_for_test` (application.c:248-273), хук подсчёта файлов
`auto_index_count_test_hook` (mcp.c:20845-20849). Фон проверяется сигналом-счётчиком, не сном.

**Отмена/shutdown:** при остановке — release+join авто-индекс джоба с дедлайном
`APPLICATION_BACKGROUND_REAP_MS=10000` (application.c:2109-2127), после — force terminate.
Отменённая сессия не может пересоздать watch (disconnect = граница владения, application.c:562-569).

### 1.2 win4r/codebase-memory-mcp-pro (`/tmp/cbm-pro`)

Тот же скелет (src/watcher, src/pipeline, src/mcp), но без daemon: `maybe_auto_index`
вызывается **синхронно в обработчике `initialize`** (mcp.c:5267) — но лишь создаёт
background-thread и возвращается, ответ не блокируется. watcher.h — та же схема git-polling
5s→60s, но: нет retry-pending, нет mutation-guard, нет статуса для клиентов, нет test seams,
`cbm_watcher_watch` возвращает void (не сообщает об отказе регистрации).
Вывод: «pro» здесь — более ранний срез; DeusData добавил ровно то, что нужно нам:
допуск-политику, single-flight, retry, наблюдаемый статус, backoff и тестовые швы.

### 1.3 /root/reactivegraph (Rust: rg-domain / rg-application / rg-adapters)

**Гибридный ленивый инжест (D026)** — `crates/rg-application/src/materialization_port.rs:1-10`:
- Per-file FSM в DB: `MaterializationState { Bootstrap, Materializing (single-flight guard),
  Complete, Stale }` (:303-320). Состояние — данные, а не память процесса: переживает рестарт.
- **Completeness-отчёт к каждому ответу** (:426-457): `Complete | Partial{total,materialized} |
  Stale{stale_files}` + `next_step_hint()` («call rg_materialize_directory...»). Запрос — только
  чтение: он сообщает полноту и что делать, но не пишет неявно. Именно так выглядит честный
  «partial results» режим.
- **Change detection по хэшу + версии экстрактора** (D046, :24-59): `FileChangeSet::compute_with_
  extractor` — unchanged только если hash совпал И extractor_version совпал; бамп версии
  перекрашивает всё, без хаков с контентом.
- **Двухфазный fast path (D035, :68-73):** дешёвый проход «список файлов + хэш» до дорогого
  парсинга — пересчёт запускается только для changed.
- **Атомарные батчи** (`BatchPolicy { batch_size: 500 }`, :27-33) — одна транзакция на батч,
  claim/release для single-flight (:622-634).
- Долгие задачи изолированы на уровне типов: ports `#[async_trait]` с первого дня
  (doc/ADR/0001-async-first.md), домен — pure sync compute; blocking-работа через
  `tokio::task::spawn_blocking` (community_port.rs:6); жизненный цикл — immutable FSM
  с историей (rg-domain/src/run_fsm.rs), бюджет — hard-fuse по размерностям
  (rg-domain/src/budget.rs).

### 1.4 Наш код (/root/prime-agent)

- `crates/pa-core/src/tools/graph.rs:61-80` — `ensure_indexed`: при первом
  search/trace/architecture, если `file_count(project)==0`, строит индекс **синхронно в запросе**
  (через `spawn_blocking`, graph.rs:691). Это уже спасает от «0 hits»-иллюзии, но первый запрос
  платит полную стоимость сборки (секунды на большом репо).
- `graph.rs:83-100` — store за `Mutex<Option<GraphStore>>`, один коннект, lazy open.
- `graph.rs:115-177` — actions: index/reindex/search/trace/architecture/coverage/changes/status;
  `status` (graph.rs:640-680) печатает nodes/edges/files — куда ляжет статус фонового индекса.
- `crates/pa-graph/src/indexer.rs:1-8` — индексатор уже инкрементальный: SHA-256 против
  `file_hashes`, skip unchanged, replace modified, remove deleted; `ignore`-walk, rayon-парсинг,
  `reindex_incremental`.
- `crates/pa-graph/src/store.rs:66-69` — **SQLite WAL + synchronous NORMAL**:
  «readers non-blocking while an index run writes» — фоновая запись не блокирует чтения,
  читатель видит консистентный снапшот до коммита.
- `crates/pa-core/src/settings/types.rs:261-314` — `GraphSettings { enabled, db_path }`;
  прецедент строкового gate: `JevSettings` `"off"|"auto"|"on"` (types.rs:44-47).
  Геттеры-расширения в `settings/manager.rs` (`get_graph_enabled`:926-936,
  `get_graph_db_path`:1109-1116).
- `crates/pa-core/src/session_engine/engine.rs:292-294, 586-607` — сессия читает гейты и строит
  `GraphTool`; это точка, где фоновая задача должна зажигаться.
- `crates/pa-daemon/src/supervisor.rs:1-3` — worker-процесс на сессию; фоновый индекс сессии
  умирает вместе с worker'ом — никакой отдельной уборки не нужно.
- `pa-daemon/src/scheduled_jobs.rs` — cron-задачи доставляются в очередь сессии на границе
  тура; расписание для индекса не нужно (см. Часть 3, вопрос «когда»).

---

## Часть 2. СРАВНИТЕЛЬНАЯ ТАБЛИЦА

| система | триггер | где живёт | как не мешает | invalidation |
|---|---|---|---|---|
| DeusData cbm | хвост 1-го ответа (`initialize`) + git-poll watcher 5s→60s | поток в daemon-процессе → supervised **subprocess** (RSS #832) | ответ не ждёт; запрос при пустой базе = structured error; глобальный pipeline-lock; батчи-транзакции | git HEAD + dirty-signature; baseline после успеха (at-least-once); backoff 2^n по отказам; prune по ENOENT-стрику |
| win4r «pro» cbm | синхронно в `initialize` (только spawn потока) | поток внутри MCP-сервера | ответ не ждёт; ошибки запросов «not indexed» | тот же git-polling watcher, без retry/статуса/гардов |
| reactivegraph | явный materialize-запрос + eager bootstrap при первом контакте | async use-case в процессе, spawn_blocking для CPU | запросы читают, **не пишут**; к ответу прилагают Complete/Partial/Stale + hint; single-flight per-file в DB | hash + extractor_version; двухфазный пересчёт (scan-hash → parse); атомарные батчи по 500 |
| prime-agent (сейчас) | первый graph-запрос (`ensure_indexed`) | `spawn_blocking` в запросе, store в `Mutex` | первый запрос платит сборку; последующие — читают WAL | SHA-256 по `file_hashes`; `reindex_incremental` по требованию |

Чему учиться: у DeusData — политика допуска + single-flight + retry + наблюдаемый статус + тестовые швы; у reactivegraph — per-file состояния в DB и честные partial/stale-ответы с hint; у себя — WAL уже даёт «фон не мешает чтению», индексатор уже инкрементальный.

---

## Часть 3. ДИЗАЙН graph=auto

### 3.1 Ответы на шесть вопросов

**1. КОГДА строить.** Два триггера, оба уже проверены практикой источников:
- *Старт сессии* (главное): в `create_session` после построения тулов — если
  `graph.enabled && graph.auto==on` — зажечь фоновую задачу первичной сборки
  (или `reindex_incremental`, если база есть: сессия перезапустилась, файлы могли уехать).
  Цель «к первому запросу готов» достигается тем, что между стартом сессии и первым graph-
  запросом модели обычно проходят секунды-минуты.
- *Первый запрос* (остаётся как есть): `ensure_indexed` — fallback, если auto off, auto
  не успел, или сессия создана до включения настройки.
- *Watch (изменение файлов)* — НЕ в v1: наш граф сессионный, между запросами у свежести
  нет потребителя (DeusData-демон шарит граф между клиентами — им watch нужен; нам — нет).
  Вместо живого watcher'а — **freshness-check при каждом graph-запросе**: дешёвая подпись
  git (HEAD + `git status --porcelain`-хэш, как DeusData) поверх кэша подписи; изменилась →
  `reindex_incremental` до ответа (он быстрый: hash-скан + точечная замена). Non-git репо —
  mtime-подпись корня проекта. Watch-фоновый режим откладываем как phase 2 (см. 3.6).
- *По расписанию* — нет. Cron-очередь сессии для этого не нужна; периодичность заменяется
  freshness-check на границах запросов.

**2. ГДЕ строить.** Фоновая `tokio`-задача **в процессе worker'а сессии**
( spawned из `create_session`), сами шаги индекса — `spawn_blocking` + rayon, как сейчас.
Не отдельный процесс: причина DeusData заводить subprocess — RSS-ratchet многопоточного
аллокатора в *долгоживущем* демоне; наша задача короткоживущая и умирает с worker'ом,
память вернётся вместе с процессом. Не в daemon-супервизоре: он не знает cwd-проекты сессий
и владел бы чужим состоянием (нарушение «one owned area per crate»: индекс — владение pa-graph
+ session engine, не daemon).
Запись идёт в **отдельном коннекте** к тому же WAL-файлу — читатели не блокируются
(store.rs:66-69), читатель видит снапшот до коммита.

**3. КАК не мешать.**
- *Приоритет*: фоновая задача не занимает запросный путь; индекс — это тот же rayon-pool,
  что и у запросного `index`, CPU-конкуренция сглаживается тем, что запросы при
  `Indexing` **присоединяются** к идущей задаче (см. ниже), а не стартуют параллельную.
- *Cold build: без partial results.* Пустой граф рождает «0 hits»-иллюзию — баг, который
  `ensure_indexed` уже чинит. Поэтому пока первичная сборка идёт, `search/trace/architecture`
  **ждут её завершения** (join через `IndexJobState`), затем отвечают по полному графу
  и добавляют одну строку «graph auto-indexed in background (N files, Xs)». Ждать — то же
  семантическое обязательство, что у текущего синхронного `ensure_indexed`, только стоимость
  обычно уже уплачена фоном.
- *Refresh: stale — можно.* Если база уже была (reindex-фаза), запрос не ждёт: WAL-снапшот
  отдаёт прошлые консистентные данные, к ответу прилагается completeness-заметка в духе
  reactivegraph: `stale: N files changed, reindex in progress` (без неявных записей).
- *Single-flight*: `ensure_indexed` и фоновая задача делят `IndexJobState`
  (`Arc<Mutex<IndexJobState>>`): `Idle | Indexing{started} | Ready{report} | Failed{error}`.
  Второй желающий строить при `Indexing` не создаёт вторую сборку, а становится в ожидание.
  Это устраняет гонку «фон строит + ensure_indexed строит второй раз в тот же проект».
- *Отмена при shutdown*: job хранит `AbortSignal`-подобный флаг/`CancellationToken`;
  worker drop / `dispose_kernel` — сигнал, задача проверяет его между файловыми батчами
  (индексатор и так пишет пофайлово). Окно утечки = один батч, процесс worker'а всё равно
  забирает память с собой. Дедлайн-реп-паттерн DeusData (join с таймаутом 10s → force) не
  нужен в процессе, который сам умирает.

**4. КАК обнаруживать изменения (v1 — на запросах, v2 — фон).**
- v1 (входит в graph=auto): кэш git-подписи `project → (head, status_sig)`; проверка
  `git rev-parse HEAD` + `git status --porcelain | sha256` (только при graph-запросе;
  ~единицы мс; не git — fold по (relpath, size, mtime) из `discover`, т.е. существующий
  ignore-walk). Отличие подписи → `reindex_incremental` → обновить кэш **после успеха**
  (at-least-once как #937). Debounce не нужен: проверка синхронна с запросом.
- v2 (настройка `graph.watch = on`, по умолчанию off): сессионный tokio-task с адаптивным
  интервалом DeusData `5s + 1s/500 файлов, cap 60s`, тот же git-signature; интервал
  пересчитывается после каждого цикла. **notify/inotify отвергнуты**: рекурсивные watch'и,
  fd-лимиты, шторм событий от `target/`/`node_modules` — и в итоге всё равно пришлось бы
  прогонять изменения через ignore-правила индексатора (дублирование источника правды).
  Git-polling дешевле, устойчивее и отражает именно то, что индексатор считает контрактом.

**5. ПАРАМЕТР.** В существующую секцию `graph` (`types.rs:261-314`):
```jsonc
"graph": {
  "enabled": true,          // как сейчас
  "dbPath": "...",          // как сейчас
  "auto": "on",             // "off" | "on";  unset читается как "off" в первом релизе
  "watch": "off"            // "off" | "on";  unset = "off"; требует auto=on (v2)
}
```
- Значения строкой по прецеденту `jev.graphRerank` (`"off"|"auto"|"on"`, types.rs:44-47);
  `auto: "auto"` — синоним `"on"`, принимается для единообразия словаря, в доке не advertised.
- Дефолт: `auto = "off"` на первый релиз (у DeusData `auto_index` тоже default false —
  автоматика, которая строит графы без спроса, обязана быть оптинной), флип в `"on"`
  отдельным PR после прожарки. `enabled=false` глушит всё, включая auto/watch.
- Типы: `GraphAutoSetting`, `GraphWatchSetting` (enum, serde rename_all lowercase);
  `GraphSettings { auto: Option<GraphAutoSetting>, watch: Option<GraphWatchSetting> }`.
  Геттеры `get_graph_auto() -> GraphAutoSetting`, `get_graph_watch() -> GraphWatchSetting`
  в `manager.rs` рядом с `get_graph_enabled`; юнит-тесты на дефолты и парсинг — рядом с
  graph_db_path тестами (manager.rs:1667-1719).

**Поток данных.**
```
create_session
  └─ gates: graph.enabled ∧ graph.auto==on
       └─ spawn IndexJob::Initial(root, project)          // tokio task, свой GraphStore-коннект
            state: Idle → Indexing
            index_project | reindex_incremental (spawn_blocking)
            → Ready{report} | Failed{err}                  // + telemetry event
graph-запрос (search/trace/arch/coverage)
  └─ IndexJobState:
       Ready ∧ sig_changed? → reindex_incremental (если watch/stale-политика) → ответ
       Indexing ∧ cold?     → await job → ответ + "auto-indexed" note
       Indexing ∧ warm?     → ответ по WAL-снапшоту + stale-заметка
       Idle/Failed ∧ пусто  → ensure_indexed (как сейчас)
status-action → + { index_state, last_index: {report|error, duration}, watching }
```

**Отношения с `ensure_indexed`.** Не заменяем — понижаем в должности до fallback-ветки
и делаем его single-flight-клиентом того же `IndexJobState` (был «проверил file_count и
строил сам», станет «проверил state; строит только если Idle ∧ пусто»). Поведение при
auto=off идентично сегодняшнему — регресс-базой служат существующие тесты graph.rs.

### 3.2 Что осознанно НЕ делаем
- Отдельный процесс/демон индексации — мотивация RSS не применима к сессионной задаче.
- partial results на холодном графе — воспроизводит «0 hits»-баг.
- inotify — см. выше; дублирование ignore-логики.
- Расписание/cron — нет потребителя вне запросов.

---

## Часть 4. РЕГРЕССИОННЫЕ ТЕСТЫ (детерминированные)

Принцип: никакого сна/retre-до-зелёного; готовность фона — **наблюдаемый сигнал**.
Тестовый шов: `IndexJobHandle` хранит `tokio::sync::watch`/oneshot, который тест
`await`-ит; в release-коде канал просто не читается (стоимость нулевая). Аналог —
атомик-счётчики DeusData (`application.c:248-273`) и наш существующий стиль
`AgentTool::execute`-тестов в graph.rs:976+.

| # | тест (уровень) | вход | ожидание |
|---|---|---|---|
| 1 | unit, `IndexJobState` | переходы Idle→Indexing→Ready/Failed; double-transition | FSM запрещает Idle→Ready, хранит последний отчёт/ошибку |
| 2 | unit, single-flight | два `ensure_indexed` при `Indexing` (второй до завершения, job-канал ещё открыт) | ровно одна сборка: `index_project` вызван 1 раз (счётчик-инъекция), второй вызов дождался того же `Ready` |
| 3 | integration, session | fixture-repo, `graph.auto="on"`, `create_session`; `await job_done_signal`; затем `graph search` | search возвращает реальные хиты, ответ **не содержит** auto-index-заметки (индекс был готов заранее); `status` показывает `index_state=ready` |
| 4 | integration, query-join | fixture + инъекция задержки в индексатор (test hook `IndexOptions`), search сразу после `create_session`, до сигнала | search возвращает полные результаты + заметку «auto-indexed in background»; после ответа state=Ready (докажет, что ждал, а не ответил пусто) |
| 5 | integration, stale-warm | индекс готов → правим файл → search без ожидания фона | ответ отдаётся (WAL-снапшот) и содержит stale-заметку с числом изменённых файлов |
| 6 | integration, отказ | `dbPath` на неписываемый путь, `auto="on"` | сессия создаётся; search внятно ругается; state=Failed{err}; повторный search не зацикливается (backoff/не-ретрай в пределах сессии) |
| 7 | settings | `auto` unset → off; `"on"`→On; `enabled=false` глушит spawn; `watch="on"` без `auto="on"` — ошибка конфига или no-op с warn (выбрать одно, задокументировать) | дефолты и парсинг, по образцу manager.rs:1667-1719 |
| 8 | shutdown | создать сессию с auto=on, сразу dispose | задача отменена (сигнал наблюдаем), процесс не паникует; join не висит |
| 9 | red-first для #2 | инвертировать single-flight (`if false && state.is_indexing()`) | тест 2 краснеет: `index_project` вызван 2×, дубликаты узлов/двойной отчёт |
| 10 | qa faux (через бинарь) | QA-репо-фикстура, prompt «найди всех callers X» в первой же реплике, `graph.auto="on"` | tool-call паттерн: `graph` использован с первого запроса, заметка авто-индекса присутствует в первом tool-result, `bash grep`-обходов нет (must/must_not как ST-*) |

Чек-лист по правилам репо: тест 4 — «фон не блокирует ответ пустотой», тест 2 — regression
на двойную сборку (баг-фикс требует тест на своём tier), всё ждёт сигналы, не sleeps.

---

## Часть 5. ПЛАН ВНЕДРЕНИЯ

Этап 1 — ядро auto (1 PR, ~1.5-2 дня):
1. `pa-core/src/settings/types.rs` + `manager.rs`: `graph.auto` enum + геттер + тесты (0.5 д).
2. Новый `pa-core/src/tools/graph_auto.rs`: `IndexJobState`, `IndexJobHandle`, spawn-функция
   (свой GraphStore-коннект, spawn_blocking, отмена токеном, тестовый done-сигнал);
   `graph.rs` остаётся < 2000 строк — вынос обязателен, не growing (0.5-0.75 д).
3. `graph.rs`: `ensure_indexed` → single-flight клиент state; cold-wait и stale-заметки;
   расширение `status`-action (0.25 д).
4. `session_engine/engine.rs`: чтение gate + spawn после построения тулов (586-607) (0.1 д).
5. Telemetry: `graph_auto_index{mode, outcome, files, duration_ms}` в схеме pa-telemetry,
   эмиссия из job — fire-and-forget, в том же PR (правило adoption telemetry) (0.1 д).
6. Тесты 1-8 + red-first прогон (0.5 д). Качественный gates: `make check`.
   PR-описание: ownership (pa-core internals + приватный модуль), classify: internals;
   parity-раздел: fork-only поверхность, TS-двойника нет — сравнение с TS-бинарем не применяется.

Этап 2 — freshness-check на запросах (~1 день): git-signature кэш + reindex_incremental
до ответа + тест 5/10; флип дефолта `auto="on"` — отдельным PR после прожарки.

Этап 3 (опционально, ~1-2 дня): `graph.watch` — сессионный poll-loop (5s→60s адаптивный,
baseline-after-success, backoff по отказам — прямо по схеме DeusData watcher.c) + тесты
с управляемыми интервалами (инъекция интервала в тестах).

Риски: (а) двойная сборка при гонке старт/запрос — закрывает single-flight (тест 2);
(б) большие репо — тот же bounded-count-приём DeusData (`auto_index_limit`, дефолт 50k,
log-warn при превышении) можно добавить в этапе 1 дёшево, политика как у них: скип + warn;
(в) WAL-размер при фоновой записи — существенные записи и так идут батчами индексатора.
