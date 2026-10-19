# LayerFS (Ephemeral-AI-Lab/layerfs) — аудит применимости к allspark

Дата: 2026-10-19. Источник: /tmp/layerfs, клон `main` @ `ddfd6acb` (pre-1.0, релиз 1.0
заявлен на 2026-10-21, "Locked until October 21"). Read-only, чужой код не менялся.

**TL;DR.** LayerFS — это версионированная ФС для агентов: один SQLite Store (CAS + CDC +
COW + дельты), ветки без копирования, FUSE-маунт воркспейса, Docker-песочницы с демоном
внутри, merge с конфликт-маркерами. Код качественный (~110k строк Rust, STRICT-схемы,
квантифицированный перф). Но: pre-1.0 lock, Linux-only (FUSE + Docker), формат может
меняться. Целиком не брать; забрать **идеи**: (1) durable CAS-журнал правок для edit-тула
(топ-1), (2) компрессия session-JSONL, (3) content-ключи для графа. Мультиагентные форки —
через git worktrees, не через LayerFS-хранилище.

---

## Часть 1. Архитектура LayerFS

### 1.1. Крейты

`core/crates/`: `layerfs-content` (31k строк: канонические объекты, CDC, деревья, merge),
`layerfs-storage` (11.4k: паки, дельты, SQLite-движок), `layerfs-persistence` (8.5k:
SQLite-схемы history/metadata/objects/lineage), `layerfs-overlay` (10.7k: оверлей
воркспейса), `layerfs-workspace` (10k), `layerfs-fuse` (5.3k, vendored fuser 0.18.0 в
`core/vendor/`), `layerfs-daemon` (6.5k), `layerfs-sandbox` (3.5k, сырой Docker HTTP по
socket), `layerfs-store-service` (10.9k), `layerfs-api` (2.2k: 29 SDK-методов),
`layerfs-bridge`, `layerfs-project`, `layerfs-history`, `layerfs-telemetry`.

Граница (core/docs/architecture/01-boundary.md:21-77): C1=`layerfs-content` чист от I/O
(идентичность+фрейминг+конструкция), C2=`layerfs-storage` владеет SQLite-файлом и паками;
между ними два трейта, `AuthenticatedObjects` (чтение канонических байтов по id,
01-boundary.md:86-144) и `FinalizedConsumer` (владение объектом, :146-160).

### 1.2. Схема хранения (вопрос A)

**Store = один SQLite-файл** (volume, общий для всех демонов; README.md:89-92 «no server
sits in the data path»). Таблицы (`core/crates/layerfs-persistence/sql/sqlite/`):

- `metadata.sql:11-18` — `object_location(object_id BLOB(32) PK, role 1..13,
  canonical_length, pack_id, group_number, record_number)`: локатор объекта внутри пака.
- `objects.sql:1-8` — `pack(pack_id, domain, digest(32), length, body BLOB)`: тела паков
  лежат прямо в SQLite (до ~16 МБ), иммутабельность принудительно триггерами
  `BEFORE UPDATE/DELETE → RAISE(ABORT)` (`objects.sql:9-13`).
- `metadata.sql:19-22` — `inode_value(ordinal PK, value BLOB(73))`: пул 73-байтовых
  инод-значений (дельты по цепочке листьев; 05-storage.md:3-45).
- `metadata.sql:23-28` — `content_signature(slot 0..8191, object_id, signature(32))`:
  кольцо подписей для поиска «этот контент уже сохранён» (дедупликация при записи).
- `metadata.sql:1-10` — `store_policy`: пороги `small_file_threshold_bytes` (128 KiB–1 MiB),
  `*_delta_max_depth` ≤ 50 для whole-file/chunk/metadata дельт.
- `history.sql` — история: `layer_stack` (:13-28, имя+scope+head_layer), `layer` (:30-66,
  цепочка слоёв с `root_id` — корень COW-дерева; `source_branch_id`+`source_commit_id`),
  `commit` (:68-109: `parent_commit_id`, **`second_parent_id`** (:81-85) — второй родитель
  мержа, `via IN (1,2,3)` (:91), message ≤ 4096 (:92), счётчики `new_canonical/new_stored/
  new_metadata` (:100-102)), `branch` (:111-136: name, `base_layer_id`, `forked_from_id`
  (:124), `head_commit_id`), `binding_hint` (:144-149, подсказки имён для мерджа).
- `lineage.sql:1-35` — `layer_lineage`/`commit_lineage`/`absorbed`/`reach`: материализованное
  родство (высоты, run-цепочки первых родителей, фронтиры публикаций) чтобы ходить по
  истории за O(сегменты), а не O(длина истории) (16-history.md:28-53).

**CAS** (content-addressing): `ObjectId = BLAKE3("layerfs/object/v2\0" ‖ канонические байты
объекта)`, 32 байта (02-objects.md:15-21; code: layerfs-content/src/object/id.rs). Канонический
конверт `LFSO`: magic+kind+длины (02-objects.md:33-43; object/codec.rs), потолок 16 MiB на
объект (:49). 13 ролей объектов (WholeFile, Chunk, ExtentLeaf/Branch, FileState, InodeL…,
02-objects.md:63-76). Идентичность покрывает фрейминг → структура аутентифицирована; при
чтении объект пересобирается и проверяется по своему id (05-storage.md:21-25).

**CDC**: FastCDC с двухбайтовым rolling GEAR-хешем, замороженный профиль: min/target/max
чанка 8/16/32 KiB, normalization shift 2, seed 0, фиксированные маски
(layerfs-content/src/file/cdc/gear.rs:13-26); идентичность профиля = дайджест всех
параметров (gear.rs:29-49). Файлы < порога (128 KiB по умолчанию) хранятся целиком одним
объектом, ≥ порога — режутся на чанки; неизменённые чанки шарятся
(85-commit-per-write-storage.md:33-36; contract/policy.rs:123).

**Дельты**: физическое представление — FULL или PREFIX/COPY+INSERT против одной прямой базы,
выбор селектором при сохранении; глубина цепочки ограничена `store_policy` (01-boundary.md:49;
encoding/delta/mod.rs:1-9 — record framing, candidates, select, chains). Пак + zstd.

**COW-деревья**: коммит переписывает только изменённые файлы и их путь по дереву; каждое
неизменённое поддерево шарится (README.md:24-27). Конструкция/обновление ФС —
`build_filesystem`/`update_filesystem` (layerfs-content/src/filesystem/update.rs:71-118),
фазы внутри (`filesystem/objects.rs`).

**Экономия**: 770 МБ файлов → 6.3 МБ БД (121×: 58× дедуп, 2.4× компрессия+дельты)
(README.md:51-52). Стоимость коммита с правкой одной строки маленького файла: ~5.1 КБ
сохранённых байт (85-…md:16-49), из них ~половина — накладные метаданные и page waste;
команды сами планируют снизить до ~3.4 КБ (85-…md:100-101).

### 1.3. Branch / fork / merge (вопрос B)

**Fork = одна строка.** «A fork copies no content and no history rows: it records one base
Layer or one selected ancestor Commit and shares everything reachable from it»
(persistence/src/history/branch.rs:1-6). `fork()` (branch.rs:30-87): проверка имени/стека,
`ForkSource::Layer` (форк от слоя) или `ForkSource::Commit` (форк от любого прошлого
коммита; обязательная проверка `ancestry_contains`, branch.rs:56-70), затем один
INSERT в `branch` (branch.rs:72-86). Fork от коммита берёт базу **того** коммита, не текущую
голову ветки (branch.rs:1-6, 47-71). Всё содержимое шарится через CAS.

**Merge — трёхпутевой, на уровне деревьев.** Вход: `MergeInput { base, ours, theirs roots,
labels, decide_from, bound }` (content/src/merge/types.rs:90-115). Решение на каждый путь —
по трём записям (decide.rs:28-31), текстовые файлы — merge3 (decide.rs:11, text.rs).
Виды конфликтов: content, add/add, modify/delete, delete/modify, type, binary, markers, mode
(types.rs:10-45). Неразрешённые конфликты **записываются маркерами в файлы воркспейса и
ничего не коммитят** (README.md:74-76; маркеры: merge/markers.rs:16-56, `has_markers`).
Плюс `/.lww` — gitignore-подобные правила last-writer-wins по mtime для указанных путей
(merge/lww.rs:1-16): путь, изменённый обеими сторонами и совпавший с правилом, берётся у
стороны с свежим mtime. История мержа: коммит с `second_parent_id` (history.sql:81-85;
16-history.md:21-26), `merge_base` по материализованному родству, tie-break для criss-cross
(16-history.md:39-43). Двухсторонние проверки: публикация против ожидаемой головы, сдвиг
головы = `HeadMoved`/`StackMoved` (persistence/src/history/merge.rs:1-7, 45-74).

**Коммит при сдвинувшейся голове**: `sandbox.commit` умеет `publish` (по ожидаемой голове)
или **merge с новой головой** и затем publish (api/src/sandbox/commit.rs:12-24; README.md:73-75).

### 1.4. Workspace mount (вопрос C)

- Воркспейс — это **не FUSE-маунт самого Store**, а оверлей: отдельная SQLite-БД на воркспейс
  («One database holds one Workspace», overlay/sql/schema.sql:1-2). Слои = `generation`
  (fs_inode:3-24, fs_entry:25-33, флаг `inherited`), незакоммиченные данные — 4 КиБ ячейки
  с bitmap validity (fs_data:34-45), трек обрезок (fs_truncate:46-53), и ключевое:
  **`fs_data_ref`** (:58-67) — «референсные куски»: диапазон файла указывает на
  уже-закоммиченный контент по `source BLOB(32)` + смещение. Незакоммиченный файл = оверлей
  поверх общих диапазонов, копирования нет.
- Сам маунт — FUSE: vendored fuser-0.18.0 (`core/vendor/`, патчи `core/patches/`),
  layerfs-fuse/dispatch/request, профиль монтирования (fuse/src/mount/mod.rs:1).
- **Где живёт демон**: в Docker-песочнице демон запускается как entrypoint контейнера
  (sandbox/src/backend/docker/container.rs:19), контейнер получает `CAP_SYS_ADMIN` + `/dev/fuse`
  (container.rs:21), Store-volume монтируется внутрь rw (`/layerfs-store`,
  container.rs:23-25), дополнительные volumes — read-only (:26-31), порт — только на host
  loopback (container.rs:33-35). То есть маунт ФС происходит внутри mount namespace
  песочницы; ядро маунтит FUSE, демон внутри читает тот же SQLite.
- Хостовые fs/branch-вызовы идут в собственном процессе клиента через локальный демон:
  JS SDK берёт `LAYERFS_DAEMON` (путь до бинарника) и `LAYERFS_VOLUME` (общий volume);
  «daemon per client… each directly opening the same Store» (sdk/js/README.md:65-78).
  Данные через сервер не ходят — все читают один SQLite.
- Commit забирает изменения: оверлей → новые канонические объекты → если ветка ушла вперёд,
  трёхпутевой merge с новой головой → атомарная публикация; конфликт → маркеры в воркспейсе,
  коммита нет (commit.rs:12-24; 85-…md:23-24 «overlay: only pieces that differ are written»).

### 1.5. «Tool call = commit» в dsh/ (вопрос D)

Механика (dsh/README.md:47-54): каждый bash-вызов агента в сессии, привязанной к проекту,
получает env `DSH_LAYERFS_PROJECT`, URL SDK и **trace-файл агента + id bash-вызова как
номер шага**; каждый завершённый SDK-вызов записывается как `plugin:layerfs/call`-рекорд в
сессию агента, плюс per-session журнал на диске. Агенты вызывают SDK из bash (короткие node
скрипты): `fs.write` — и это уже коммит.

Ключевой факт: **`fs.write`/`fs.rm`/`fs.apply` публикуют по одному коммиту на вызов**
(85-commit-per-write-storage.md:9-12) — «commit-per-write». Дешёвка коммита (~5 КБ, §1.2)
делает это осмысленным. Это даёт: точечный rewind (форк от любого коммита — branch.rs:56-70),
replay истории, MCTS-rollouts (форк от состояния перед действием), blame по строкам
(README.md:77-78 «blame traces each line to the Commit»), и полный аудит «кто и каким
вызовом». UI: dsh рисует ветки/воркспейсы/коммиты вживую (dsh/README.md:83-109).

### 1.6. Replay/rewind (вопрос E)

Уточнение по STATS.md: «replay» там — это **нагрузочный прогон**: история реального репозитория
воспроизводится командами агентов по диапазонам (dsh/tools/history/report.mjs:1-9 — метрики
по попыткам, гейты, бейзлайны). Итог 15 диапазонов: 19,974 SDK-вызова → 10,547 коммитов,
52 ветки, 30/30 плановых конфликтов отработаны (STATS.md:29, 33-49). Это верификация
продукта, а не фича для пользователя. Пользовательский rewind = форк от любого прошлого
коммита (README.md:56-58) + восстановление произвольного файла из дерева коммита
(`GET .../tree?ref=<saved id>`, dsh/README.md:68-71).

### 1.7. Конкурентность и надёжность

Один глобальный SQLite, один писатель: WAL + directory-lock между процессами + очередь
допуска внутри процесса + «publication lane» на атомарный шаг «проверить голову, смержить,
опубликовать»; читатели не блокируются (83-store-concurrency-model.md:11-30). Ставка: коммиты
редки (секунды латентности модели между вызовами), оптимизируется неконкурентный случай
(:23-26). Коммит-пер-запись в нагрузочном прогоне: 3.7 commit/s средний (STATS.md:14-23).

---

## Часть 2. Пять вопросов для allspark

### Вопрос 1. Multi-agent parallel edits (fork workspace каждому агенту)

**Вердикт: адаптировать паттерн, НЕ переносить механизм. Git-worktree вместо LayerFS.**

- Проблема реальна: у нас подагенты rlm.spawn работают в одной ФС; параллельные правки
  одного файла = тихая порча или гонка. У LayerFS это решено архитектурно (форк = одна
  строка БД, merge с маркерами, README.md:69-76).
- Но перенос их хранилища = принятие ~50k строк (content+storage+persistence+overlay+fuse)
  + FUSE-зависимость + Linux-only + формат «may change without notice» до 1.0
  (README.md:22-25). Для 17-крейтного форка это чужой архитектурный слой.
- Дешёвый эквивалент для нас: **ветка+worktree на подагента**, которые реально пишут:
  `git worktree add ../wt-<agent> -b allspark/agent/<name>` (мс-секунды, без дублирования
  .git), ребёнок правит в worktree, по завершении родитель делает merge; конфликт →
  маркеры + репорт родителю (прямой аналог LayerFS-поведения). Всё средствами git, ноль
  нового формата хранения, работает на macOS.
- Риски: (а) индексация графа в worktree (см. вопрос 3); (б) проекты без git — откат к
  текущему поведению; (в) `node_modules`/build-артефакты в worktree дублируются по диску
  (LayerFS тут выигрывает у git); (г) работа только для детей с записью — research-детей
  трогать не надо.
- Оценка ценности: средняя. У нас большинство параллельных детей читают, а не пишут.
  Писателей обычно 1-2, и родитель оркестрирует. Конфликты — редки, но дороги (тихая порча).

### Вопрос 2. Tool call = commit для edit-тула

**Вердикт: дополнить, не заменять. Durable CAS-журнал правок поверх существующего EditStore.**

Сравнение с нашим:
- `EditStore` (pa-edit/src/store.rs:195+): in-memory, `record()` → хэш-тегированные снапшоты
  по пути, `versions()/by_hash()/head()`, лимиты `max_paths/max_versions/max_total_bytes`,
  `fork()/commit_from` для батчей. Это быстрый интерактивный слой — **оставить как есть**.
- Сессия JSONL уже аудит (каждый tool call записан). Но: неструктурирован для вопросов
  «покажи файл X на 14:32», пережимается плохо, переживает только вместе с сессией.
- LayerFS показывает, что коммит на каждое мелкое изменение стоит ~КБ, если хранить
  content-addressed + дельты (§1.2), и что это даёт rewind/replay/blame почти бесплатно.

Переносимая форма для нас — **журнал правок (edit journal)**: SQLite-БД (~per-user, рядом с
~/.prime/agent): таблица объектов `(hash BLOB PK, bytes BLOB zstd, base_hash, role)` +
таблица правок `(id, ts, session_id, agent, path, before_hash, after_hash, tool, mode,
stats)`. Каждый применённый edit → одна запись (до/после по хэшу). Отсюда бесплатно:
undo/rewind любого файла на любую точку, cross-session, diff-любых-двух-состояний,
«кто изменил эту строку» (edit-blame), статистика (new_canonical/new_stored как в
history.sql:100-102). Хэш-теги hashline-снапшотов связываем с журналом (tag → journal hash).
Хранение: идентичные состояния дедуплицируются по хэшу (правка и откат = 0 новых байтов),
последовательные версии — дельта (наш pa-diff уже есть).

- Не берём commit-per-write ФС (нам не нужна ФС-версионированность целиком) и не трогаем
  латентный путь EditStore: журнал пишется асинхронно (fire-and-forget; наш же принцип
  телеметрии).
- Оценка: см. Часть 3.

### Вопрос 3. graph=auto × LayerFS

**Вердикт: не применимо в лоб; забрать принцип content-ключей, если пойдём в форки.**

- Если агенты работают в worktree-форках: индексировать каждый форк = дублирование работы и
  N SQLite-графов. Решение в духе LayerFS: **индекс ключуется по content-hash**, не по пути:
  символы/ребра считаем от BLAKE3 содержимого файла; форк с неизменённым файлом получает
  индекс бесплатно (CAS-дедуп на уровне записи графа). Изменённые файлы переиндексируются
  точечно. Путь-агностичный граф живёт один на проект.
- Стоимость: переделка ключей pa-graph (пути → (path, content_hash)), инвалидация по хэшу.
  Само по себе полезно и без форков (фон-переиндексация пропускает неизменённые файлы —
  у нас и так WAL/rayon, но сравнение хэшей дешевле парсинга).
- Если форки не делаем (см. вопрос 1) — отдельной работы не требуется; текущий индексатор
  в рабочей копии остаётся корректным.

### Вопрос 4. CAS/CDC для session logs

**Вердикт: адаптировать как компрессию-at-rest, без LayerFS-форматов.**

- Наши данные: 844 JSONL-файла, 392 МБ, топ-файлы 3–8 МБ (subagent-транскрипты). Повторы —
  внутри одного файла и между sub/parent (одинаковые tool-выходы, повторный контекст).
- CDC (8–32 КиБ чанки) на наши записи (~0.5–8 КиБ) не рассчитан — выгода не от чанкинга.
  Реальные рычаги: (а) zstd с обученным словарём на JSONL — обычно 10–20× на агентских
  логах; (б) дедупликация крупных повторяющихся строк (tool-выходы > 8 КиБ) по хэшу с
  back-reference — по сути CAS для значений; (в) опционально дельта против предыдущего
  вхождения. Ожидаемо 392 МБ → 15–30 МБ (≈13–25×), сопоставимо с их 121× на ФС, где
  дедуп доминирует.
- Очередь: писать фреймами (блок N записей, независимый zstd-фрейм) — хвост сессии при
  падении теряется не целиком; append-only сохраняем; читатель понимает и голый JSONL
  (обратная совместимость). Пишется вне paint-пути (сессия — не критический путь рендера).
- Честная оговорка: 93 МБ из ~/.prime/agent/sessions и сотни МБ артефактов — диск дешёвый;
  ценность средняя, работы мало. Делать после edit-журнала.

### Вопрос 5. Sandbox isolation (Docker-песочницы для bash-тула)

**Вердикт: не применимо сейчас (как дефолт); опциональный бэкенд — сильно позже, если вообще.**

- У LayerFS песочница оправдана моделью: контейнер = disposable compute, состояние в Store,
  демон внутри с `/dev/fuse` + `CAP_SYS_ADMIN` (container.rs:12-36). Наше позиционирование
  иное: allspark работает в среде пользователя с его тулчейном; требовать Docker = ломать
  установку у большинства; latency `docker exec` на каждый bash-вызов; ops-поверхность
  (образы, сокеты, volumes) не соответствует «harness в терминале».
- Если когда-нибудь понадобится изоляция выполнения — дешевле собственные механизмы
  (landlock/seccomp на Linux, sandbox-exec на macOS, opt-in) или один долгоживущий
  контейнер на сессию с bind-mount cwd. Но это отдельный проект, LayerFS тут не донор —
  их Docker-бэкенд (3.3k строк сырого HTTP к докер-сокету, одноразовые попытки, никакой
  адаптации) заточен под их демон-в-контейнере и FUSE.

---

## Часть 3. Трудозатраты

| Кусок | Объём | Часы | Комментарий |
|---|---|---|---|
| 2. Edit journal (CAS + дельты, SQLite+zstd) | новый модуль в pa-edit + surfacing в TUI | **40–60 ч** | запись асинхронная; GC по возрасту/размеру; QA: unit (pa-edit), интеграционный на reopen/crash; телеметрия события в том же PR |
| 4. Сессия-JSONL компрессия (zstd-фреймы + словарь + дедуп крупных значений) | pa-core writer/reader seam | **24–40 ч** | обратная совместимость чтения; бенч на pa-perf корпусах |
| 1. Fork-per-subagent через git worktrees + merge-on-done | pa-core (spawn-слой) + qa-tasks | **60–100 ч** | только для пишущих детей; fallback без git; флаг; real-model QA на конфликт |
| 3. Content-ключи в pa-graph | pa-graph | **20–40 ч** | нужен только если принят пункт 1; самостоятельная польза умеренная |
| 5. Docker-бэкенд bash | — | **80–120 ч** | не рекомендуется (см. Часть 2.5) |
| Перенос самого LayerFS-хранилища/FUSE | — | **500+ ч** | не рекомендуется: pre-1.0 lock, Linux-only, чужой формат |

Практичный пакет: **пункты 2 + 4 ≈ 64–100 ч**, дают durable rewind/audit + −90% диска сессий,
без FUSE/Docker/новых форматов. Пункт 1 — следующий, по доказанному спросу (qa-задача,
которая краснеет на гонке правок).

## Часть 4. Топ-1 для немедленного портирования

**Durable CAS-журнал правок (вопрос 2), 40–60 ч.** Почему:
1. Усиливает существующий тул (edit), а не добавляет подсистему; не трогает критический путь.
2. Закрывает реальную боль: сейчас undo живёт в памяти процесса (EditStore) — рестарт/крэш
   воркера теряет историю; журнал даёт cross-session rewind, audit и edit-blame.
3. Дешёв в хранении по их же измерениям: контент-addressed дедуп + дельта → правка ≈ КБ
   (85-…md:16-49); у нас вдобавок pa-diff уже есть.
4. Не тянет зависимостей: SQLite (уже в стеке через pa-graph), zstd, BLAKE3.
5. Композируется с будущим пунктом 1: журнал правок в worktree-форке = готовая база
   merge-помощника и аудита мержей.

## Часть 5. Чего НЕ брать

1. **FUSE-слой и демон-маунты** (layerfs-fuse, vendored fuser, daemon/control): Linux-only,
   пользовательские репозитории на нашей ФС — риск стабильности и латентности; нам нужен
   rewind истории правок, а не сетевая ФС.
2. **Формат Store и всю history-модель** (layer_stack/layer/commit_lineage/runs/absorbed/reach):
   построено под commit-per-write многих агентов; для нашего журнала хватает двух таблиц;
   формат заморожен только с 1.0 (README.md:22-25), до этого — чужая миграционная нагрузка.
3. **Docker-песочницы как дефолт** (§2.5): не наш продукт-контур.
4. **`/.lww` mtime-LWW-мердж**: для кода опасен (свежий mtime перезаписывает содержимое);
   git-style merge3 с маркерами и ручным разбором — правильнее. У них LWW — уступка
   агентной эргономике, нам она не нужна.
5. **SDK/JS-слой и dsh-плагин**: чужой хост (DeepSeek Harness); у нас своя TUI и wire.
6. **Однописательник + directory-lock + очередь** (83-…md): нужен только из-за «много
   процессов на одном SQLite»; у нас на запись журнала один владелец-процесс на юзера —
   обычного WAL достаточно.
