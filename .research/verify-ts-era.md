# TS-era кросс-чек: находки ревью против истории upstream

Дата: 2026-10-16. Ревью-артефакты: `/tmp/compound-engineering-0/ce-code-review/20261016-230055-3df17b3e/` (synthesized-findings.json, upstream-issues.json).
TS-референс: клон тега `v0.9.8` (PrimeIntellect-ai/prime-agent, 7d442aafa, 2026-09-29) в `/tmp/prime-ts-ref`, только чтение.

## 0. Таймлайн и главный факт об issue-трекере

| Событие | Дата | SHA |
|---|---|---|
| v0.9.8 — последний TS-релиз | 2026-09-29 | 7d442aafa |
| Port Prime Agent to Rust (#2524) смержен в main | 2026-09-29/30 | 39bc99a91d |
| v0.10.0 — первый Rust-релиз | 2026-10-09 | — |
| Форк (rager306/allspark) отделился от upstream/main (merge-base) | 2026-10-13 | 33da15a2c |
| upstream/main HEAD (форк НЕ содержит эти 2 дня) | 2026-10-15 | a01c7923cf |
| HEAD форка | 2026-10-16 | 375ad95cf |

**Главный факт:** из 15 закрытых issues семи групп только **#3363 закрыт как COMPLETED**. Остальные 14 — **NOT_PLANNED**, из них 12 с комментарием vouch-бота («issue reporters must be vouched … will be closed automatically»): #1606, #1645, #1665, #1718, #2016, #2542, #3082, #3332, #3365, #3447, #3474, #3592, #3594, #3617, #3644. То есть **«closed» ≠ «починено»**: у большинства фиксы либо отсутствуют вовсе, либо смержены позже без ссылки на issue. Вердикты ниже проверены по коду TS-тега и коду форка, а не по state.

Второй факт: `prime-agent-runtime/src/rlm/repl.py` форка **байт-в-байт идентичен upstream/main** (sha256 5b97d430…, 75146 байт; TS v0.9.8 — 57570 байт, другой код). Всё, что в runtime Python, форк наследует 1:1 с upstream.

---

## A. Семь upstream-групп

Формат колонок: вердикт ревью → мой вердикт. «До тега» = фикс присутствует в коде v0.9.8.

### 1. context-truncation-compaction — ревью: FIXED → **подтверждаю с оговорками**

| Issue | Даты | Upstream | TS v0.9.8 | Rust-форк |
|---|---|---|---|---|
| #3363 end_turn при max_tokens | 10-05 → 10-06, **COMPLETED** | фикс PR #3367 (9981e9250, 2026-10-06, «acp: map a truncated final response to the max_tokens stop reason») | баг есть (маппинга нет) | фикса в форке: `acp_mode_e2e.rs:1733` (тест с refs #3363), `anthropic/convert.rs:311` `max_tokens → Length` |
| #1718 противоречия каталога → silent truncation | 08-24, NOT_PLANNED (vouch) | фикса нет | reconciliation отсутствует | симптомы закрыты: Length-маппинг, `COMBINED_LIMIT_HEADROOM_FLOOR` = `max(reserve_tokens, floor)` (compaction.rs:147-158, «TS checks only contextWindow - reserveTokens»), overflow compact-and-retry (`overflow_compaction.rs`) |
| #1645 %-порог компакции | 08-22, NOT_PLANNED (vouch) | фича не сделана | `shouldCompact`: абсолютный `reserveTokens` | тоже абсолютный `reserve_tokens` (+floor) |

Итог: **FIXED-в-Rust** для #3363 (единственная группа с настоящим COMPLETED-фиксом). #1718 — закрыт только класс симптомов, data-level противоречия каталога не решены ни в одной эре. #1645 — фича не реализована ни в TS, ни в Rust; метка «FIXED» для неё некорректна.

### 2. daemon-auth-oauth-refresh — ревью: FIXED (era: Rust) → **подтверждаю код, но не «закрытие»**

| Issue | Даты | Upstream | TS v0.9.8 | Rust-форк |
|---|---|---|---|---|
| #3617 auth.json читается один раз; 8h OAuth expiry | 10-14, NOT_PLANNED (vouch) | фикса-коммита с refs нет | тот же класс: per-process ModelRegistry/AuthStorage (main.ts:511), кэш инстанса освежается только своими записями (auth-storage.ts:765-904) | фикса есть **с port-коммита** (39bc99a91d, 2026-09-29): свежий `AuthStorage::create` на каждый turn-preflight (`turn/model.rs:44-50`), `refresh_oauth` по `expires` + single-flight `refresh_flight` (`lookup.rs:65-88,149-186`) |
| #1606 403 model-group травит весь credential | 08-21, NOT_PLANNED (vouch) | фикса нет | частично ограничено до тега: маркировка stale **по source-токенам** провалившегося запроса (`_markProviderAuthStale`, agent-session.ts:13280-13297) | per-resolution lookup + staleness-гейт |
| #2542 version gates новых моделей | 09-22, NOT_PLANNED (vouch) | фикса нет | фикс **до тега**: комментарий-фикс client-version gate (`providers/anthropic.ts:85` «The API gates newer models on the claimed client version») | перенос |

Итог: **FIXED-в-Rust, не регрессия**. Код в форке — upstream-наследие (все три поверхности датированы port-коммитом ≤2026-09-29 < branch-point). Эра «Rust» верна для #3617; но issue закрыт ботом, «FIXED» следует читать как «код в порте покрывает класс», а не «upstream закрыл фиксом».

### 3. provider-retry-failover-routing — ревью: RECURS → **подтверждаю ровно для #3312**

| Issue | Даты | Upstream | TS v0.9.8 | Rust-форк |
|---|---|---|---|---|
| #3312 failover держит armed route | **OPEN** с 10-02 | не чинен | тот же класс (mutable `_runModel()` в guard, agent-session.ts:13347) — см. раздел B | баг жив: `turn/model.rs:363-367`; blame — upstream (eba4a6504, 2026-10-13 ≤ branch-point), **не форк-регрессия** |
| #3365 429 → child completed | 10-06, NOT_PLANNED (vouch) | фикса нет | баг жил (репорт «Observed on 0.9.8 (main @ ebc07508d)») | класс закрыт: `settle_failed` с армами Prompt/Unreachable, статус `error`, failure-строка родителю (`rlm_children/lifecycle.rs:630-708`) |
| #3332 ordered fallback chain | 10-03, NOT_PLANNED (vouch) | фича-запрос | был только одиночный `providerBackupModel` (settings, agent-session.ts:13339-13350) | мульти-кандидатный retry.failover есть (шире TS) |

Итог: **RECURS верен только для #3312** (атрибуция routed-image эпизода). #3365 и #3332 в форке закрыты/превзойдены. Попутно из #3312: `providerBackupModel` парсится в Rust-настройках, но читателя в `crates/` нет — TS-0.9.8-фича молча игнорируется портом (кандидат на deprecation-warning).

### 4. kernel-snapshot-file-truncation — ревью: RECURS → **подтверждаю, рецепт готов**

| Issue | Даты | Upstream | TS v0.9.8 | Rust-форк (== main) |
|---|---|---|---|---|
| #3082 restore молча трunc'ает файлы | 09-29 (день тега), NOT_PLANNED (vouch, «closed one second after it opened» — из тела #3447) | частичный фикс позже: PR #3346 (672085e88, 2026-10-06) | **баг полностью жив**: `_restore_state` → голый `dill.loads(blob)`, guard отсутствует; save-side не пропускает файловые хэндлы | `refuse_raw_fd` отказывает **только int-fd** (repl.py:1080-1085), именованные 'w'-хэндлы проваливаются в `create_filehandle(name, *args)` с O_TRUNC; `_ALWAYS_SKIP` = только harness-имена (rlm, mcp, bash, asyncio, …) |
| #3447 named 'w' всё ещё опустошает файлы | 10-09, NOT_PLANNED (vouch) | **фикса НЕТ** — закрыт ботом; последний коммит по repl.py = #3346 | — | баг жив в main == форк |

Итог: **TS-баг-перенесён-верно + недочинен upstream**. Это не регрессия порта: TS-эра имела баг целиком, Rust-эра добавила int-fd guard (#3346) и остановилась. Рецепт фикса — см. раздел B.

### 5. session-resume-registry-leases — ревью: FIXED → **код есть, но происхождение в ревью указано неверно**

| Issue | Даты | Upstream | TS v0.9.8 | Rust-форк |
|---|---|---|---|---|
| #3592 «fix: ACP session resume — registry race, archive fallback, system_instructions strip» | 10-14, NOT_PLANNED (vouch) | **это issue, а не PR**: ничего не смержено под этим номером | — | перечисленное в форке ЕСТЬ, но от upstream-коммитов ≤ branch-point: insert-before-spawn (`supervisor/worker_lifecycle.rs:343-345`), live-binding conflict naming (`create_reuse.rs:332-335`, «TS assertWorkerCreateOwner»), `session_archive.rs` |
| #3314 resume после bridge termination; ownerClientId | **OPEN** с 10-02 | не чинен | угол жив (ownerClientId-механика без очистки при bridge-терминации) | наследует; регресс-теста нет (согласен с ревью) |
| #1078 ghost-сессии/leases | 08-15, NOT_PLANNED (без vouch-комментария) | — | фикс **до тега**: eviction пустых сессий worker'а при detach последнего клиента (`daemon-supervisor.ts:1472` «Evicted empty session worker … on last client detach»), idle-eviction sweep | перенос |

Итог: **FIXED-в-Rust по коду**, но утверждение ревью «FIXED by merged #3592» фактически неверно: #3592 — issue, закрытый vouch-ботом; код появился из других upstream-коммитов. #3314 — открытый угол в обеих эрах.

### 6. cron-scheduler-persistent-goals — ревью: FIXED (era: TS) → **подтверждаю по коду тега, не по трекеру**

| Issue | Даты | TS v0.9.8 (код в теге) | Rust-форк |
|---|---|---|---|
| #1665 cron-классы недостижимы из package root | 08-23, NOT_PLANNED (vouch) | подтверждено: `export class AgentCronJobStore/AgentCronScheduler` есть в `core/cron-jobs.ts:194/1004`, но **не реэкспортированы** из `src/index.ts` | `pub mod cron` в `pa-core/src/lib.rs:61` — публичная поверхность есть. FIXED-в-Rust |
| #1313 goals дурно деактивируются после retry exhaustion | 08-15, NOT_PLANNED (vouch) | механизм **до тега**: bounded provider wait с экспоненциальным backoff вместо сдачи (`agent-session.ts:13362-13367` «keep pinging with bounded exponential backoff instead of giving up») | `goal_continuation.rs` / `goal_state_persist.rs` |
| #2016 hot-loop на неблокирующей длинной работе | 09-03, NOT_PLANNED (vouch) | механизм **до тега**: `_goalContinuationAwaitsRlmWork` / `_autonomousContinuationAwaitsRlmWork` — таймерные продолжения ставятся на паузу, пока идёт RLM-работа (комментарий в agent-session.ts) | deliberate divergence: failure backoff (`cron/scheduler.rs:3-23` «cron-jobs.ts re-fires a failing job at full [rate]» — осознанное расхождение с TS) |

Итог: **FIXED** — #1313/#2010-класс закрыт ещё в TS-транке до тега (по коду), #1665 закрыт только портом. Эра «TS» верна для #1313/#2016, для #1665 — это фикс порта.

### 7. installer-updater-tls-roots — ревью: FIXED → **подтверждаю; источник — upstream PR, не issues**

| Issue | Даты | Upstream | Rust-форк |
|---|---|---|---|
| #3474 launcher пропадает после удаления TS npm-пакета | 10-10, NOT_PLANNED (vouch) | — | вне TS-скопа (инсталлер-скрипты форка); staged-update planner есть |
| #3594 updater не сверяет версии | 10-14, NOT_PLANNED (vouch) | фикс без refs на issue: version gate в порте с port-коммита + PR #3442 «support TypeScript updates to Rust» (c174739ad, 2026-10-09) | `update_flow/plan.rs:183` `is_release_update_candidate` + `--force`, rollback; blame: 39bc99a91d |
| #3644 non-public root CA | 10-15, NOT_PLANNED (vouch) | фикс PR #3477 «Trust system and custom certificate authorities» (8fd44c25b, 2026-10-12) < branch-point | `utils_inner/tls.rs:45-49`: native certs + EXTRA_CERTS + webpki; тест custom_ca_native_roots |

Итог: **FIXED-в-Rust**. Все три issue закрыты ботом, но фактические фиксы (#3442, #3477, port-коммит) смержены до branch-point форка и присутствуют.

---

## B. Два RECURS: был ли фикс в TS-транке после v0.9.8 и готовый рецепт

TS-транка после v0.9.8 не существует как живая ветка: порт #2524 смержен на main на следующий день после тега, TS-файлы удалены. Поэтому «фикс TS-транка после тега» = (а) коммиты в `prime-agent-runtime` (Python, общая для обеих эр) и (б) сторонние форки v0.9.8.

### B1. #3447 — kernel restore O_TRUNC (P1, repl.py GuardedUnpickler)

История: TS v0.9.8 — guard'а нет вовсе (голый `dill.loads`, снапшот сериализует ВСЕ пользовательские имена, включая файловые хэндлы) → #3082 (2026-09-29, день тега) закрыт ботом без триажа → #3346 (2026-10-06) добавил `refuse_raw_fd` для int-fd + `_ALWAYS_SKIP` для harness-имён → #3447 (2026-10-09) показал неполноту (named 'w'-хэндлы) и закрыт ботом. **После v0.9.8 полный фикс не появлялся** (последний коммит по repl.py — #3346). Баг жив в upstream/main == форк: save-side не пропускает `io.IOBase` (только `_`-префикс и `_ALWAYS_SKIP`), restore-side пропускает именованные хэндлы в `create_filehandle`.

**Рецепт ГОТОВ и протестирован репортёром #3447** (dill 0.4.1, CPython 3.11):
1. Save-side: не пиклить файловые объекты — skip любых `io.IOBase` с записью в `skipped` (reason `"file handle"`).
2. Restore-side: расширить guard — для mode, содержащего `w/a/x/+`, не открывать реальный путь:
   ```python
   def safe_create_filehandle(name, mode, position, closed, open_, strictio, fmode, fdata):
       if isinstance(name, int):
           raise pickle.UnpicklingError(f"refusing to reopen raw file descriptor {name} from a snapshot")
       if any(c in mode for c in "wax+"):
           return create_filehandle(os.devnull, mode, 0, True, open_, strictio, fmode, "")
       return create_filehandle(name, mode, position, closed, open_, strictio, fmode, fdata)
   ```
   (ревивится закрытый хэндл на /dev/null; файл сохраняет байты). Пункт 2 обязателен: он защищает уже лежащие на диске снапшоты.
3. Альтернатива из #3447: `dill.CONTENTS_FMODE` (fmode=1) при dump — открывает с O_CREAT без O_TRUNC; репортёр отверг как менее безопасный (воссоздаёт удалённые файлы, оставляет живые write-хэндлы на пользовательских путях).

Действие: **чинить самим** по рецепту 1+2 (Rust-сторона правится в `prime-agent-runtime/src/rlm/repl.py` — общий файл с upstream, дифф минимальный) + кандидат в upstream issue (репорт уже существует, но убит vouch-ботом; в новом репорте сослаться на #3447/#3082).

### B2. #3312 — failover switch держит armed image-route (P2, turn/model.rs:363-367)

- Код в форке — upstream-наследие (blame eba4a6504 #3393, 2026-10-13 ≤ branch-point). **Не форк-регрессия.**
- TS-эра: тот же класс жил в v0.9.8 — backup eligibility сравнивалась с мутабельным `_runModel()` (agent-session.ts:13347); комментарий 13339-13344 показывает частичную осведомлённость (image-route duplicate), но полный фикс не смержен.
- Сторонний TS-фикс: **thomaswillner/prime-agent PR #24** (OPEN, не смержен, база `release/v0.9.8` = тег 7d442aafa):
  1. eligibility сравнивать с моделью, **обслужившей провалившийся message** (`message.provider/model`), а не с текущей selection;
  2. сессия, уже ушедшая с упавшего маршрута, немедленно переиздаёт turn (`auto_retry_start.reason = "selected"`), selection главнее более раннего backup-hop, ничего не восстанавливать после;
  3. каждый zero-delay re-route тратит одну попытку `retry.maxRetries` (бюджет проверяется до re-route);
  4. +4 теста в `agent-session-retry-events.test.ts`, RED на непропатченном источнике.
- Upstream-рецепт из тела #3312 (Solution): switch в routed episode обязан ставить `next` (route-aware: сохранить image override, сменить provider), учесть `Retry-After` провайдера, события называть реального получателя. Плюс: тест «armed route + scripted primary failure → `provider_target.model` == backup candidate».
- Rust-порт уже не имеет неограниченной части (`MAX_TOTAL_PROVIDER_RETRIES` перед каждым switch) — осталась только атрибуция.

Действие: **чинить самим** по рецепту выше (готовых upstream-коммитов нет: #3312 OPEN, twn#24 не смержен). Фикс — кандидат в upstream PR.

---

## C. Parity-чувствительные находки: класс каждой

| # | Находка | TS v0.9.8 (проверено по коду тега) | upstream/main после v0.9.8 | Класс |
|---|---|---|---|---|
| 4 | @file-инжест без капа | `cli/file-processor.ts`: текстовый путь — `readFile(absolutePath, "utf-8")` целиком + verbatim `<file name=…>` в промпт, **капа нет**; гейт есть только у картинок (resize/omit) | TS удалён; Rust `file_processor.rs` — портированное поведение | **TS-баг-перенесён-верно** → кандидат upstream issue (чинить = deliberate divergence: кап байтов + маркер усечения, как у image-пути) |
| 13 | Оценка токенов картинок при компакции | `compaction.ts estimateTokens`: user — считаются **только text-блоки** (image → 0); toolResult/custom — `image → chars += 4800 // ~1200 tokens` (flat). TS-тестов на image-оценку нет (в compaction*.test.ts упоминаний 4800/image нет) | — | **TS-баг-перенесён-верно** — вплоть до комментария «~1200 tokens» и нулевых user-картинок. Кандидат upstream issue; фикс (размер по data.len()/4) = divergence |
| 11 | traceback/error мимо 64 KiB капа | Host-side settle капает **только stdout/stderr/result/background** с маркерами (`kernel/repl-manager.ts:1210-1221`); `error` (ename/evalue/traceback) уходит целиком (:1231) и склеивается в текст (`tools/ipython.ts:729`). Kernel-side: `_RESULT_TEXT_CAP = 1_048_576` **на строку** traceback/evalue (коммит 0e3c90022 «cap error text», 2026-09-22, до тега) — т.е. «кап» есть, но 1 MiB/строку, а не 64 KiB на всё | repl.py идентичен форку | **TS-баг-перенесён-верно** (асимметрия host-капа и 1 MiB per-line кап сохранены портом). Многострочный traceback в сотни раз превышает бюджет. Кандидат upstream issue; фикс в Rust (append_truncated на evalue+traceback в events.rs/execution.rs) = divergence, согласовывать с upstream |
| 6 | PKCE verifier как `state` | `packages/ai/src/utils/oauth/anthropic.ts:252` — `state: verifier`; вся state-проверка (callback server, paste-парсинг) сравнивает с verifier | `crates/pa-ai/src/oauth/anthropic.rs` **идентичен форку** (`("state", verifier)`, комментарий «kept for wire parity») — после v0.9.8 не чинили | **TS-баг-перенесён-верно; жив в обеих эрах** → кандидат upstream issue. Оговорка: перед фиксом проверить, не требует ли Anthropic authorize-endpoint именно state==verifier для этого CLI-флоу (wire-parity комментарий намекает); sibling-флоу codex уже mint'ит независимый state — паттерн есть |

---

## Выводы: какие грабли уже чинены upstream и что оттуда брать

1. **Грабля №1 — доверять state issues.** 12 из 15 закрытий семи групп — авто-закрытия vouch-бота (NOT_PLANNED), ещё 2 — NOT_PLANNED вручную. Только #3363 закрыт фиксом. Любой « upstream уже починил» надо проверять кодом тега/транка, как здесь.
2. **Уже починено upstream и присутствует в форке** (merge-base 33da15a2c, 2026-10-13): #3363 (PR #3367), kernel int-fd guard (#3346), TLS roots (#3477), TS→Rust update gate (#3442 + port-коммит), ACP-resume registry/archive (upstream-коммиты без issue-refs), provider-wait/goal-continuation (TS-транк до тега). Ничего подтягивать не нужно, кроме двух дней upstream/main после branch-point (a01c7923cf — не смотрел, вне скоупа).
3. **Живые в обеих эрах — чинить самим, кандидаты в upstream:**
   - #3447 named-'w' restore (P1, потеря данных) — рецепт готов в теле #3447, применить 1+2;
   - #3312 failover attribution (P2) — рецепт: тело #3312 (Solution) + twn PR #24 (не смержен);
   - @file без капа (#4), image-токены компакции (#13), traceback мимо 64 KiB (#11), PKCE state (#6) — верные породы TS-багов, у upstream рецептов нет.
4. **Форк-специфичных регрессий среди проверенного нет**: оба RECURS — upstream-наследие (blame ≤ branch-point; repl.py байт-идентичен main), все четыре parity-находки — точный порт TS v0.9.8.
5. **Поправка к ревью-артефакту:** «FIXED by merged #3592» неверно — #3592 это issue (NOT_PLANNED, vouch-бот), код резюма/registry в форке от других upstream-коммитов.

### Допущения
- Датировки TS-фиксов #1313/#2016/#1078/#2542 установлены по наличию кода в теге v0.9.8 (depth-1 клон не даёт TS-истории коммитов); точные TS-коммиты не назывались.
- #3474 (launcher после удаления npm-пакета) проверен только по Rust-инсталлеру; TS-сторона вопроса не кодилась (npm-эра кончилась).
- «TS-транк после v0.9.8» трактован как: Python-runtime (общий) + сторонние форки тега; сам upstream-main после тега — уже Rust.
