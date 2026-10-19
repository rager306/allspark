# МАТРИЦА ТЕСТИРОВАНИЯ allspark

Изучены: **prime-agent** (~5500 тестов) и **daily-archive** (~5700 тестов). Ниже —
синтез лучших практик обоих + матрица тестов для 142 фишек allspark.

---

## 1. Как тестируют в prime-agent

### 1.1 Масштаб (inline `#[test]`)
| Крейт | test-файлов | inline тестов |
|---|---|---|
| pa-tui | 97 | 1587 |
| pa-core | 58 | 1286 |
| pa-daemon | 136 | 1258 |
| pa-cli | 60 | 637 |
| pa-ai | 8 | 383 |
| pa-types | 5 | 172 |
| pa-models | 6 | 75 |
| pa-telemetry | 1 | 68 |
| pa-agent | 2 | 29 |

### 1.2 Ключевые паттерны prime
| Паттерн | Где | Суть |
|---|---|---|
| **Golden-replay parity** | `pa-core/tests/golden_replay.rs` + `tests/golden/corpus/*.json` | Реплей корпуса от РЕАЛЬНОГО TS → байт-идентичный вывод Rust. `deep_norm` нормализует tmp-пути/hash. |
| **Prompt-guards** | `pa-core/tests/prompt_guards.rs` | `selected_tools: Some(vec!["python_repl"])` явно → новые tools не ломают. |
| **Kernel lifecycle e2e** | `pa-core/tests/kernel_*.rs` (15 файлов) | prewarm/teardown/restore/snapshot/join/revive. |
| **Daemon e2e** | `pa-daemon/tests/*_e2e.rs` (67 файлов) | полные сценарии (create/join/reap/abort/fork-isolation). |
| **proptest** | `pa-tui/view/geometry.rs`, `pa-daemon/supervisor/tests.rs` | property-based (geometry, supervisor). |
| **CI sharding** | `.github/workflows/ci.yml` | `cargo test --workspace` шардирован; sibling-binary parity. |
| **Windows triage** | `windows-runtime-triage.yml` | `--test-threads=1` для flaky Windows. |

### 1.3 Известные flaky-тесты prime (важно!)
- `refinement::tests::concurrent_update_harness_state_writes_all_land` (~50% flaky)
- `request_timing::payload::tests::concurrent_producers_drop_at_the_reserve_when_the_queue_is_full`
- `golden_bash_group_matches_ts` (git-version hash drift)
- `decision_runtime_tests::decision_api_preimports*` (флаки под параллельной нагрузкой)

---

## 2. Как тестируют в daily-archive

### 2.1 Масштаб
| Крейт | inline тестов |
|---|---|
| da-parse | 1892 |
| da-cli | 1132 |
| da-application | 953 |
| da-domain | 900 |
| da-adapters | 370 |
| kg-storage | 155 |

### 2.2 Ключевые паттерны daily-archive (отличия от prime!)
| Паттерн | Кол-во | Где | Суть |
|---|---|---|---|
| **Contract-тесты** | **115** | `*_contract.rs` | Контракт-как-тест: архитектурные gate, batch/cache/route-контракты |
| **Shadow-тесты** | 24 | `*_shadow_test.rs` | Параллельный прогон нового vs старого, сравнение |
| **Invariants** | 11 | `*invariant*` | Семантические инварианты стадий |
| **Acceptance/journey/e2e** | 22 | `*acceptance*`, `*journey*` | Полные пользовательские пути |
| **Provenance** | — | `*provenance*` | Лицензии, консервация, происхождение |
| **Governor** | 15 YAML | `scripts/governor/checks/*.yaml` | Декларативные контракты + positive/negative fixtures |
| **Arch-gate (static)** | — | `architecture_contract.rs`, `verify_rust_architecture.sh` | Сканирует Cargo.toml/исходники: запрет зависимостей (onion-архитектура) |
| **test_fidelity_drift** | — | `checks/test_fidelity_drift.yaml` | Ловит «код зелёный, но гейт врёт» (композиция компонентов) |

### 2.3 Ключевые идеи daily-archive
1. **Контракт-как-тест** (115!): тест читает Cargo.toml + исходники и падает при нарушении архитектурного правила. Пример: `da-parse` не должен зависеть от `Samyama/kg-*/SONA/redb`.
2. **positive/negative fixtures**: каждый governor-чек имеет пару JSON — валидный и невалидный. Проверяет, что гейт ловит нарушение.
3. **test_fidelity_drift**: fixture-запуск на известных метриках; отклонение → drift_risk. Ловит регрессии композиции, которые юнит-тесты пропускают.
4. **Shadow-тесты**: новый путь прогоняется рядом со старым, результаты сравниваются (миграция без регрессий).
5. **Governor (мета-слой)**: `governor/fixtures/` + `scripts/governor/checks/*.yaml` — контроль отделён от данных.

---

## 3. СИНТЕЗ: 8 классов тестов для allspark

| Класс | Источник | Применение в allspark |
|---|---|---|
| **T1 Golden-parity** | prime | байт-идентичность при default-off (upstream) |
| **T2 Contract-gate** | daily-archive | архитектурные: unsafe=forbid, FORK-ONLY маркеры, слой-зависимости |
| **T3 Differential** | оба | closure-repair: full==incremental |
| **T4 Shadow** | daily-archive | новый LSP/граф рядом со старым (rg/Python) |
| **T5 Positive/Negative fixtures** | daily-archive | guards (stale/seen/noop), provider_errors |
| **T6 Property-based** | prime (proptest) | граф-инварианты, name_path, diff |
| **T7 e2e (real LSP)** | prime (daemon e2e) | rust-analyzer/pyright/gopls |
| **T8 Metrics-drift** | daily-archive | graph_recall, lsp_call_ms, coverage |

---

## 4. МАТРИЦА ТЕСТОВ ПО ФАЗАМ

| Фаза | T1 | T2 | T3 | T4 | T5 | T6 | T7 | T8 |
|---|---|---|---|---|---|---|---|---|
| **Ф0** (pa-diff/ast/lsp-core) | — | ✅ unsafe/слой | — | — | ✅ | ✅ diff/name_path | ✅ LSP spawn | — |
| **Ф1** (LSP-ядро) | ✅ schema | ✅ | — | ✅ vs rg | ✅ diag-identity | ✅ name_path | ✅ rust-analyzer | ✅ lsp_call_ms |
| **Ф2** (граф+поиск) | ✅ | ✅ | — | ✅ vs rg | — | ✅ граф-инварианты | ✅ | ✅ recall@k |
| **Ф3** (closure-repair) | — | — | ✅ **full==incremental** | — | — | ✅ случайные правки | — | ✅ full_rebuild_count |
| **Ф4** (edit+guards) | ✅ edit-parity | ✅ | — | ✅ vs Python-edit | ✅ **stale/seen/noop** | ✅ | ✅ | ✅ guard_hits |
| **Ф5** (клей) | — | — | ✅ did_change цепочка | — | ✅ | ✅ | ✅ e2e edit→diag | ✅ diag freshness |
| **Ф6** (память/оптимизации) | — | — | ✅ eviction | — | ✅ cache-invalidation | ✅ | — | ✅ cache_hit_rate |
| **Ф7** (config) | ✅ default-off | ✅ изоляция | — | — | ✅ toolset-layering | ✅ | ✅ | — |
| **Ф8** (опции) | — | ✅ | — | — | ✅ | ✅ | — | — |

---

## 5. КОНКРЕТНЫЕ ТЕСТЫ (по фишкам)

### T2 Contract-gate (новый класс для allspark, из daily-archive)
| Гейт | Проверяет | Источник идеи |
|---|---|---|
| `architecture_contract.rs` | unsafe=forbid; новые крейты (pa-lsp/pa-ast/pa-graph) не тянут pa-core; FORK-ONLY маркеры | daily-archive arch-gate |
| `fork_isolation_contract.rs` | default-off → байт-идентичность upstream; 5 точек вторжения помечены | prime prompt_guards + daily arch |
| `prompt_budget_contract.rs` | число tool-схем ≤ N; суммарный prompt ≤ бюджет | — |

### T3 Differential (главный тест инкрементальности)
```rust
proptest! {
  fn closure_repair_matches_full_rebuild(edits in prop::collection::vec(arb_edit(), 1..50)) {
    let inc = apply_incremental(&edits);
    let full = rebuild_from_scratch(&edits);
    prop_assert_eq!(inc.nodes, full.nodes);
    prop_assert_eq!(inc.edges, full.edges);
  }
}
```

### T5 Positive/Negative fixtures (guards, из daily-archive)
```
tests/fixtures/guards/stale_tag.negative.json     # ожидаем ошибку
tests/fixtures/guards/stale_tag.positive.json     # ожидаем успех
tests/fixtures/guards/seen_lines.negative.json
tests/fixtures/guards/noop_3x.negative.json
tests/fixtures/guards/binding_mismatch.negative.json
```
Каждая фикстура: `{"input": {...}, "expect": "error|ok", "violation": "..."}`.

### T7 e2e с реальным LSP
```
tests/e2e/lsp_rust_analyzer.rs   # definition/references/hover/rename на фикстуре
tests/e2e/lsp_pyright.rs
tests/e2e/lsp_gopls.rs
tests/e2e/edit_then_diagnostics.rs  # правка → did_change → новые диагностики
```

### T8 Metrics-drift (из daily-archive test_fidelity_drift)
```yaml
# checks/graph_fidelity.yaml
watched_sources: [crates/pa-graph/src/indexer.rs, crates/pa-ast/src/block.rs]
fixture: "fixtures/repos/trench-small"
expected: {nodes_min: 400, nodes_max: 450, edge_precision: 0.95, recall_at_10: 0.9}
```

---

## 6. ПРИОРИТЕТЫ ТЕСТОВ (что писать первым)

| # | Тест | Фаза | Почему первым |
|---|---|---|---|
| 1 | **T2 architecture_contract** (unsafe/слой) | Ф0 | Дёшево, ловит главный риск (forbid-unsafe, merge-конфликт) |
| 2 | **T1 golden-parity default-off** | Ф1 | Гарантия upstream-совместимости |
| 3 | **T7 e2e rust-analyzer** | Ф1 | LSP — сердце; серверы установлены |
| 4 | **T3 differential closure-repair** | Ф3 | Ловит все дыры инкрементальности разом |
| 5 | **T5 guards fixtures** | Ф4 | Red-first, готовый образец (pa-edit parity) |
| 6 | **T8 metrics-drift (graph)** | Ф2 | Детект деградации индекса |
| 7 | **T6 property name_path** | Ф1 | Rust-имена с `/`, generics, юникод |
| 8 | **T4 shadow vs rg** | Ф2 | Граф vs текущий rg/Python |

---

## 7. ИНФРАСТРУКТУРА (что перенять)

| Элемент | Из | Куда в allspark |
|---|---|---|
| golden-корпус + `deep_norm` | prime | `pa-graph/tests/golden/` |
| contract-тесты (115) | daily-archive | `pa-*/tests/*_contract.rs` |
| governor-YAML + fixtures | daily-archive | `scripts/governor/checks/*.yaml` |
| test_fidelity_drift | daily-archive | `checks/graph_fidelity.yaml` |
| proptest | prime | граф/name_path/diff |
| CI sharding | prime | `.github/workflows/ci.yml` |
| `--test-threads=1` для flaky | prime | Windows-пути |

---

## 8. ЧЕК-ЛИСТ ФАЗЫ (definition of done)

Каждая фаза allspark закрывается, когда:
1. ✅ **Unit** — фишки фазы покрыты inline-тестами.
2. ✅ **Contract** — архитектурный гейт (unsafe/слой/маркеры) зелёный.
3. ✅ **Parity** — при default-off байт-идентичность upstream.
4. ✅ **Negative** — каждый guard/ошибка имеет negative-fixture.
5. ✅ **e2e** — сценарий фазы проходит на реальном сервере (LSP) или фикстуре.
6. ✅ **Metrics** — метрики фазы в норме (drift-check).
7. ✅ **No flaky** — тест не в списке flaky (или помечен).

---

## 9. ВЫВОДЫ

**prime-agent даёт:**
- golden-replay parity (байт-идентичность с TS) — критично для upstream-совместимости
- e2e daemon-тесты (67 файлов) — образец для LSP-сценариев
- proptest + CI sharding

**daily-archive даёт:**
- **contract-тесты (115)** — архитектурные гейты (unsafe/слой/маркеры) — **главное новое**
- **positive/negative fixtures** — для guards
- **test_fidelity_drift** — детект «зелёный, но врёт»
- **shadow-тесты** — миграция без регрессий
- **governor** — мета-слой контроля

**Синтез для allspark:**
- T2 contract-gate (из daily) — **первый тест** (дёшево, ловит forbid-unsafe/merge)
- T1 golden-parity (из prime) — upstream-совместимость
- T3 differential closure-repair — главный тест инкрементальности
- T5 fixtures (из daily) — guards red-first
- T8 metrics-drift (из daily) — деградация графа/LSP
