# ReactiveGraph — анализ применимости к allspark

Источник: /root/reactivegraph (Rust, 4 крейта, ~3MB, redb+RVF embedded store,
30-тульный MCP). Философия: **deterministic FSM оркестрирует, WASM исполняет,
LLM — вызываемая capability, никогда не оркестратор**.

> Замечание: в прошлом синтезе reactivegraph был представлен тезисно (per-file FSM
> + completeness-отчёт). Это — полный разбор, с file:line, и трезвая оценка
> переносимости каждого механизма.

---

## 1. Их ключевые механизмы

### 1.1 Per-file FSM materialization (ядро, переносимо)

`crates/rg-application/src/materialization_port.rs:356` — `FileState`:
```rust
pub struct FileState {
    repo_id, relative_path, file_hash: String,
    state: MaterializationState,
    extractor_version: String,   // D046: версия экстрактора!
}
```
`MaterializationCompleteness` (:423): `Complete | Partial{total, materialized} |
Stale{stale_files}` + `next_step_hint()` — **статическая строка следующего шага
для агента** ("call rg_materialize_directory to update").

**Суть**: каждый файл знает своё состояние materialization, и любой read-only
запрос возвращает ответ ВМЕСТЕ с completeness-отчётом — агент видит, полное ли
перед ним picture, и что делать дальше, без триггера неявной записи.

### 1.2 RunFSM — детерминированный оркестратор

`crates/rg-domain/src/run_fsm.rs:162`: `Created→Planning→Executing→Verifying→
Completed/Failed`. Валидация переходов (:326), terminal-проверка, JSON round-trip,
property-тесты (mutation testing убивает bypass).

### 1.3 Budget как hard-fuse контракт

`doc/01-architecture.md:248`: "Budget is a pure rg-domain hard-fuse contract.
Empty/unbounded budgets are INVALID; at least one explicit limit; monotonic
exhaustion semantics across per-call, per-task, per-agent, per-workflow,
per-day". Композируется с инвариантом nucleus «bounded latency, no unbounded
loops».

### 1.4 Anti-fantasy 4 уровня (ADR-0011)

1. Claim: `PROPOSED → VERIFIED` только с GroundingKind (property-тест убивает bypass)
2. Graph: каждая мутация через `MutationGate::apply_proved` + witness log
3. Output: `VerifierResult{schema_ok, citation_coverage, goal_coverage,
   constraint_compliance, replay_hash_stable}`
4. Action: `VerifiedToolResult{result, readback}` — независимое перечитывание после invoke

### 1.5 MCP_COMPARISON.md — dashboard честности

Таблица «нас vs GitNexus vs cbm vs graphify» на одном транспорте, с nodes,
freshness GREEN/стале, и **ловушками каждого** (например: cbm "moderate mode
excludes rg-composition/bin — our binary callers appear as 2-caller results; not
a parity failure"). Плюс прямое признание: cbm `get_architecture` "rich and
proven — reuse it".

### 1.6 Hexagonal layering H1–H4 внутри L2–L5

Порты в domain, адаптеры наружу, зависимости только внутрь. Per-file state живёт
в H2 (application), persistence — H3 (adapters), композиция — H4.

---

## 2. Сопоставление с нашим кодом

| Их механизм | Наш аналог | Разрыв |
|---|---|---|
| `FileState{hash, state, extractor_version}` | `pa-graph` `file_hashes` (SHA-256, changed/modified/added/removed) | **extractor_version у нас нет** — смена парсера не помечает граф stale |
| `MaterializationCompleteness{Complete/Partial/Stale}` + hint | `coverage` (index honesty report) + `TraceResult.truncated` | **нет per-query completeness**: search/trace не говорят «частичный/устаревший» |
| RunFSM с валидацией переходов | нет (компакция/адмиссия — свои состояния, не FSM) | осознанно: у нас другие жизненные циклы |
| Budget hard-fuse (per-call/task/agent/day) | `Budget{tokens, steps, timeout_s}` в qa-tasks | у нас только в QA; в продукте — tool-loop детектор в бэклоге |
| `VerifiedToolResult{result, readback}` | `unsupported_document_changes_write_nothing` (валидация до записи) | **readback после записи есть не везде** — точечное усиление |
| MCP comparison dashboard | нет | наш аналог — QA manifest, но без «traps» колонки |

---

## 3. Вердикт по каждому механизму

### 3.1 Per-file FSM + Completeness + next_step_hint — ПЕРЕНЯТЬ (8-16ч)
**Куда**: `pa-graph/src/store.rs` (таблица `file_states` с `extractor_version`) +
`pa-core/src/tools/graph.rs` (completeness в ответ search/trace/changes).

**Семантика**: после `ensure_indexed`/`reindex` каждый файл в `Complete`; при
`changes` обнаружено изменение → файл помечается `Stale` (hash сменился), ответ
несёт `Completeness::Stale{stale_files}` + hint «call graph reindex». `Partial`
— когда индекс ещё строится.

**Почему**: это закрывает класс «graph ответил из устаревшего индекса и не
сказал» — тот же fail-open класс, что мы чинили в safe_delete. Сейчас наш
`coverage` отвечает только на явный запрос, а per-query completeness делает
честность дефолтом.

### 3.2 `extractor_version` — ПЕРЕНЯТЬ (2-4ч)
Смена версии экстрактора/грамматик tree-sitter должна помечать весь граф stale
одним полем, а не «повезёт/не повезёт» после переразбора. У нас грамматика в
wasm-пакетах с pinned RELEASE — версия уже детерминирована, осталось её
записывать.

### 3.3 Budget hard-fuse — ПЕРЕНЯТЬ КОНТРАКТ (4-8ч)
`Budget` в `rg-domain` как тип с инвариантом «пустой = невалидный» + monotonic
exhaustion. У нас budgets живут в qa-tasks YAML; контракт уровня типа сделал бы
их носимыми в продукт (tool-loop детектор из Codex-трека получит тип бесплатно).

### 3.4 `VerifiedToolResult{result, readback}` — ЧАСТИЧНО ПЕРЕНЯТЬ (4-8ч)
Независимый readback после записи: для `edit` — перечитать файл и сравнить с
ожидаемым post-state (у нас уже есть expected-state, не хватает явного readback
в результате). Для `lsp.rename` — readback = re-grep старого имени (0 совпадений
= успех). Осторожно: readback на каждый вызов удорожает тул; делать флагом или
только для мутирующих действий.

### 3.5 RunFSM — НЕ ПЕРЕНИМАТЬ
Наши жизненные циклы (turn, compaction, admission breaker) уже смоделированы
своими состояниями; общий FSM добавил бы абстракцию без наблюдаемой выгоды.
Property-тесты переходов — хорошая идея, но к нашим сущностям, не к их.

### 3.6 MCP comparison dashboard — ПЕРЕНЯТЬ ИДЕЮ (4-8ч)
«Таблица честности» в `.research/`: pa-graph vs gitnexus vs cbm на одном
транспорте, с nodes/freshness/traps каждого. Мы уже собираем это вручную в
отчётах — формализовать.

### 3.7 redb/RVF, WASM skills, witness log, 9router — НЕ ПЕРЕНИМАТЬ
redb vs наш SQLite WAL — равная цена, разная экосистема; WASM-sandboxskills —
другая модель расширения (у нас skills/python + native tools); witness log —
требует crypto-подложку ruvector; 9router у нас уже есть как провайдер.

---

## 4. Топ-1: Completeness envelope для graph-ответов

Самый ценный перенос, потому что он **закрывает последний открытый fail-open
графа** — «ответ из устаревшего индекса, неотличимый от актуального». Всё
остальное (safe_delete, trace.truncated, changes.unindexed) мы уже закрыли;
completeness — последний класс. И он усиливает `graph=auto`: фоновая сборка
меняет `Partial → Complete` естественно, и агент видит прогресс.

Дизайн:
- `pa-graph`: `FileState{path, hash, state, extractor_version}` в новой таблице
  (или расширение `file_hashes`)
- `pa-graph`: `Completeness{Complete, Partial{n,m}, Stale{k}}` + `next_step_hint()`
- `pa-core graph.rs`: search/trace/changes несут `completeness` в details и
  одну строку в тексте
- Тесты: red-first (инверсия state → тест ловит), mutation на extractor_version

## 5. Отсечённое (кратко, чтобы не возвращаться)

RunFSM как общий оркестратор; redb/RVF замена SQLite; WASM skill packs; witness
log без ruvector; 9router-специфика (у нас провайдер); их 30-тульный MCP как
рывок «больше тулов» (наш принцип — меньше тулов, богаче описания).
