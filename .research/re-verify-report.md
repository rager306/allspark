# RE-VERIFY: 14 свежих коммитов форка на HEAD

**Дата:** 2026-10-17
**Репозиторий:** `/root/prime-agent`, ветка `feat/isolated-defaults`
**Диапазон:** `375ad95cf..74d1e3d04` — 14 коммитов
**HEAD на момент прогона:** `cfb29fb94` (14 коммитов задачи + `cfb29fb94 docs(fork): readme-fork covers the jev layer…`, только `readme-fork.md`, кода не касается)
**Окружение каждого прогона:** `PA_TS_BINARY=/nonexistent/ts`, `CARGO_PROFILE_DEV_DEBUG=0`, `DO_NOT_TRACK=1`; ambient `PRIME_AGENT_CODING_AGENT_DIR` / `SESSION_DIR` / `DAEMON_SOCKET` и `GH_TOKEN` / `GOOGLE_*` сняты (см. «Методика»).
**Дерево:** чистое (`git status --porcelain` пуст) до и после прогона; все мутации откатывались.

---

## Итоговая таблица

| sha | коммит | файлы ок? | тест(ы) | результат | расхождения |
|---|---|---|---|---|---|
| `2b3d74842` | fix(allspark): worker-socket-closed prompts stay consumed | ✅ 4/4 в зоне (`pa-tui`: daemon_client, errors, prompt, tests) | `-p pa-tui --test daemon_rejection_headless` (10) + `-p pa-tui --lib prompt_delivery` (2, вкл. `worker_socket_closed_refusal_is_ambiguous_delivery`) | ✅ 10 passed / 2 passed | нет |
| `2e79330c3` | fix(allspark): classify in-stream payment signals, bound provider requests | ✅ 5/5 (`pa-ai` stream/stream_tests/mock_server/http + `pa-core` provider_adapter) | `-p pa-ai --lib in_stream_credit_frame_classifies` (1), `a_signal_present_request_still_times_out` (1); `-p pa-core --lib every_stream_attempt_carries` (1) | ✅ 1 / 1 / 1 passed | нет |
| `ffd91a66b` | fix(allspark): https-only installer env URLs, fork-safe stop candidates | ✅ 6/6 (`pa-cli` discovery + 2 tests, `pa-core` installer, `pa-daemon` paths, `install-rust.sh`) | `-p pa-core --lib update::installer` (15), `-p pa-daemon --lib paths` (7), `-p pa-cli --lib daemon_discovery` (30), `-p pa-cli --test installer_platform_map` (20) | ✅ 15 / 7 / 30 / 20 passed | нет (см. прим. 1) |
| `58394f09a` | fix(allspark): cap kernel error payloads on settle | ✅ 4/4 (`pa-ai` stream_failure tests, `pa-core` kernel execution/shared, tests/kernel_error_clip) | `-p pa-core --test kernel_error_clip` (3) + `-p pa-ai --lib truncates_raw_payload_on_multibyte` (1) | ✅ 3 / 1 passed | нет |
| `734261a93` | fix(allspark): compaction rebuilds and telemetry off the UI critical path | ✅ 6/6 (`pa-cli` telemetry, `pa-tui` run/session_ui/apply/lifecycle + test) | `-p pa-tui --test loop_rpc_spawned_headless` (`a_slow_compaction_rebuild_does_not_starve_input`) | ✅ 1 passed | нет |
| `5461ba80a` | fix(allspark): cap @file text embeds at 256 KiB | ✅ 1/1 (`pa-cli/src/file_processor.rs`) | `-p pa-cli --lib file_processor` (12; 4 целевых: oversized / at-cap / giant line / multibyte) | ✅ 12 passed | нет |
| `07364009a` | fix(allspark): geometry-aware image tokens in compaction estimates | ✅ 3/3 (`pa-ai` lib/stream, `pa-core` compaction) | `-p pa-core --lib compaction` (85; вкл. `user_images_estimate_by_geometry_not_zero`) | ✅ 85 passed | нет |
| `0f7547061` | fix(allspark): independent OAuth state for Anthropic PKCE | ✅ 2/2 (`pa-ai` oauth/anthropic + oauth/pkce) | `-p pa-ai --lib the_verifier_never_rides` (1), `the_state_is_an_independent` (1) | ✅ 1 / 1 passed | нет |
| `abe2a419b` | fix(kernel): neutralize write-mode file handles on snapshot restore | ✅ 3/3 (`prime-agent-runtime` repl.py/repl.md/test_repl.py) | `uv run --with pytest python -m pytest test/test_repl.py -k 'neutralizes or read_only'` → 3 теста | ✅ 3 passed (весь файл: 146 passed) | нет |
| `37e788625` | test(core): pin golden corpus git dates to explicit UTC offsets | ✅ 1/1 (`tests/golden/corpus/bash.json`, 7/7 дат) | `-p pa-core --lib golden_bash_group` (1) | ✅ 1 passed, вкл. **TZ=UTC / Asia/Krasnoyarsk / America/New_York** | нет |
| `9c6ca829d` | feat(allspark): jev decision layer, phase 1 | ✅ 22/22 (новый `pa-jev`, `pa-core/src/jev`, settings, engine, Cargo) | `-p pa-jev` (22: 11 lib + 11 integration) + `-p pa-core --lib jev` (23, вкл. `create_session_installs_jev_tool_only_when_enabled`) | ✅ 22 / 23 passed | нет |
| `5122da3d3` | test(cli): e2e harnesses isolate ambient PRIME_AGENT_* env | ✅ 4/4 (только `pa-cli/tests/*`) | `--test acp_mode_e2e acp_daemon_attached` (17), `--test continue_guard_e2e` (2), `--test daemon_commands_e2e` (5), `--test differential_cli` (3) | ✅ 17 / 2 / 5 / 3 passed, **и с живым ambient-окружением** | нет (см. прим. 2) |
| `f6328d9e8` | docs(fork): fork-identity under the launcher isolation model | ✅ 1/1 (`docs/fork-identity.md`) | — (docs) | ✅ n/a — проверен `--stat`; содержимое сверено с `AGENTS.md` и `scripts/allspark-launcher.sh` | нет |
| `74d1e3d04` | style: workspace rustfmt and clippy hardening | ✅ 73/73, только `crates/pa-{graph,lsp,mem,types,telemetry,agent,cli/tests}`; `pa-edit`/`pa-models`/`pa-diff`/`pa-ast` не тронуты | — (style) | ✅ `cargo fmt --all --check` → rc 0; `cargo check --workspace --all-targets` → 0 errors / 0 warnings (не-vendor); `cargo clippy --workspace --all-targets` → только vendor `crossterm` | нет (см. прим. 3) |

**Все 14/14 коммитов: GREEN.**

---

## Red-proof: тесты действительно ловят регресс

Каждый ключевой тест прогнан против **откаченной** правки (содержимое родителя коммита при сохранённых новых тестах) — тест обязан покраснеть. Результаты:

| коммит | мутация / откат | краснеет? |
|---|---|---|
| `2b3d74842` | `git checkout 2b3d74842^ -- prompt.rs daemon_client.rs errors.rs` | ✅ `worker_socket_closed_refusal_keeps_the_draft_consumed` FAILED (9 passed / 1 failed) |
| `2b3d74842` | `is_worker_socket_closed_rejection` → `if false` | ✅ юнит `worker_socket_closed_refusal_is_ambiguous_delivery` FAILED |
| `2e79330c3` | `git checkout 2e79330c3^ -- anthropic/stream.rs` | ✅ `in_stream_credit_frame_classifies_as_payment_required` FAILED |
| `2e79330c3` | вернуть старую ветку `(Some(signal), _)`, отбрасывающую `timeout_ms` | ✅ `a_signal_present_request_still_times_out_on_a_stalled_peer` FAILED (паника на 5-секундном bound) |
| `2e79330c3` | `timeout_ms: Some(PROVIDER_REQUEST_TIMEOUT_MS)` → `None` | ✅ `every_stream_attempt_carries_a_bounded_request_timeout` FAILED |
| `ffd91a66b` | `if is_https_url(url) \|\| allow_http` → `if true` | ✅ 2 теста воронки FAILED |
| `ffd91a66b` | ослабить `is_upstream_ts_daemon_socket` (`is_some_and(\|_\| true)`) | ✅ `the_upstream_ts_daemon_socket_is_recognized_by_path_only` FAILED |
| `ffd91a66b` | убрать отказ в `daemon_discovery` | ✅ `the_upstream_ts_daemon_is_never_a_stop_candidate_for_a_fork_root` FAILED |
| `ffd91a66b` | `install-rust.sh`: всегда `ts_socket_probe="yes"` | ✅ `the_fork_run_skips_the_upstream_ts_socket_and_probes_the_fork_socket` FAILED |
| `58394f09a` | `git checkout 58394f09a^ -- kernel/{manager/execution,shared}.rs` | ✅ 2 из 3 FAILED (`error_fields_and_traceback_clip_on_char_boundaries`, `megabyte_error_repr_clips_at_the_default_cap`) |
| `58394f09a` | убрать `error.ename = clip_at_chars(...)` | ✅ `error_fields_and_traceback_clip_on_char_boundaries` FAILED |
| `734261a93` | `git checkout 734261a93^ --` 5 файлов UI-петли | ✅ `loop_rpc_spawned_headless` FAILED |
| `5461ba80a` | `take(total_bytes.min(CAP))` → `take(total_bytes)` + оба `if` → `if false` | ✅ 3 из 4 целевых FAILED (oversized / giant line / multibyte) |
| `07364009a` | `user_image_tokens` → `0` (TS-паритет) | ✅ 3 теста FAILED (`…by_geometry_not_zero`, `…elision_marker`, `…shift_the_context_estimate`) |
| `0f7547061` | `authorization_url(challenge, state)` → `verifier` | ✅ `the_verifier_never_rides_the_authorization_url` FAILED |
| `abe2a419b` | `_WRITE_MODE_FLAGS = frozenset("wax+")` → `frozenset("")` | ✅ 2 из 3 pytest FAILED (write/append neutralization) |
| `37e788625` | `git checkout 37e788625^ -- bash.json` | ✅ `golden_bash_group_matches_ts` FAILED (на этой машине, TZ=+07) |
| `9c6ca829d` | `should_install_jev_tool`: `Off => false` → `chain.is_some()` | ✅ `the_tool_gate_off_by_default_and_auto_requires_native` FAILED |
| `9c6ca829d` | `Auto => chain.is_some_and(starts_native)` → `chain.is_some()` | ✅ тот же тест FAILED (native-only правило закреплено) |

Замечания к red-proof:
- `5461ba80a`: откат файла на `5461ba80a^` **не** краснит — вместе с правкой уходят и новые тесты (8 passed). Поэтому доказано мутациями: удаление чтения-с-лимитом и удаление маркера дают ровно 3 красных теста из 4 целевых, а «файл ровно в кап» остаётся зелёным (он и не должен ловить).
- `0f7547061`: мутация «`state = verifier`» в `login_anthropic` **не** краснит `the_verifier_never_rides…` — тест вызывает `login_with_server` напрямую со своим `state`, т.е. покрывает провод (`authorization_url`), а не минтинг в `login_anthropic`. Мутация `generate_state()` → `generate_pkce().0` тоже не краснит `the_state_is_an_independent…` (сравнение с *другим* случайным verifier). Обе дыры — в тестах, не в коде: сам продовый путь независимого state корректен и закреплён тестом провода + `parse_paste`-тестами. См. «Наблюдения» п. B.
- `9c6ca829d`: мутация `if jev_enabled != Off` → `if true` в `engine.rs` **не** краснит `create_session_installs_jev_tool_only_when_enabled` — гейт внутри `should_install_jev_tool` всё равно возвращает `false` для `Off`, так что тест зелёный по другой причине. Внешний гейт — избыточная защита (не даёт строить registry), тест его не различает. См. «Наблюдения» п. C.

---

## Методика (важно для воспроизведения)

1. **Env-дисциплина.** Все прогоны: `PA_TS_BINARY=/nonexistent/ts CARGO_PROFILE_DEV_DEBUG=0 DO_NOT_TRACK=1`. Дополнительно пришлось снимать **ambient-переменные живой сессии агента** — иначе тесты `pa-daemon`/`pa-cli` читают реальный `~/.prime/agent`:
   - `PRIME_AGENT_CODING_AGENT_DIR`, `PRIME_AGENT_SESSION_DIR`, `PRIME_AGENT_DAEMON_SOCKET`, `PRIME_AGENT_INTERNAL_*`;
   - `GH_TOKEN`, `GOOGLE_APPLICATION_CREDENTIALS`, `GOOGLE_CLOUD_PROJECT`, `GOOGLE_CLOUD_LOCATION` — без этого `get_model_catalog` видит `github-copilot`/`google-vertex` в `configuredProviders` и 3 теста `pa-daemon` краснеют на **любом** коммите (включая родителя). Это ambient-артефакт машины, не регресс.
2. **Kache/target.** Общий `CARGO_TARGET_DIR` между основным деревом и временным worktree родителя (`/root/wt-375`) **портит артефакты** (kache отдаёт объекты по хешу, но не по пути): `pa-core` начал падать с `cannot find function image_tokens_for_dimensions`. Лечение — `cargo clean -p pa-ai -p pa-core` и пересборка. Все red-proof после этого делались **через `git checkout <sha>^ -- <файлы>` в основном дереве** (с гарантированным откатом), а не через отдельный worktree.
3. **Мут-тесты** выполнялись in-place: файл → мутация → прогон → восстановление из сохранённой копии; после каждого шага `git status --porcelain` проверялся на пустоту.

---

## Наблюдения (не блокеры для upstream-подготовки)

**A. `2e79330c3` — тест держит 5-секундный внешний bound.** `a_signal_present_request_still_times_out_on_a_stalled_peer` заворачивает `send()` в `tokio::time::timeout(5s)`. Это осознанно (иначе «зависание» не отличить от медленного ответа), но на сильно загруженной машине тест теоретически может флапнуть. За 5 прогонов — стабильно зелёный.

**B. `0f7547061` — покрытие минтинга, а не только провода.** Тест `the_verifier_never_rides_the_authorization_url` вызывает `login_with_server(..., &state)` со **своим** `state`, поэтому регресс «вернуть `state = verifier` в `login_anthropic`» он не поймает (проверено мутацией). Продовый код корректен (`let state = generate_state();` + `AnthropicCallbackServer::start(&state)`), но защита от повторного скатывания — только через код-ревью. Кандидат на follow-up: тест уровня `login_anthropic` (или assertion, что `state != verifier` в `parse_paste`-пути).

**C. `9c6ca829d` — внешний гейт в `engine.rs` не покрыт.** `if jev_enabled != Off` — избыточен относительно `should_install_jev_tool` (тот же предикат внутри). Тест `create_session_installs_jev_tool_only_when_enabled` зелёный и при `if true`, потому что внутренний гейт всё равно отсекает `Off`. Не дефект, но «red without the gate» из body относится к **внутреннему** гейту, а не к внешнему.

**D. `ffd91a66b` — `daemon_discovery` (30) и `pa-daemon paths` (7), а не 5.** В задании указано «`-p pa-daemon --lib paths` (5)». Фильтр `paths` матчит и `rlm_ledger::tests::*paths*` — отсюда 7. Все 7 зелёные; целевой `the_upstream_ts_daemon_socket_is_recognized_by_path_only` — внутри. Аналогично `pa-cli --lib daemon_discovery` = 30 (совпадает с заданием).

**E. `5122da3d3` — контракт skip’а распространён не на все differential-харнессы.** `differential_cli.rs` и `daemon_commands_e2e.rs` теперь трактуют «`PA_TS_BINARY` задан, но отсутствует» как **skip**. А `lock_compat_e2e.rs` (`ts_binary()` → `Some(PathBuf::from(path))`) и `package_e2e.rs` — **нет**: с `PA_TS_BINARY=/nonexistent/ts` они падают на `timeout: failed to run command '/nonexistent/ts'` (exit 127). Это ровно тот класс поломки, который коммит чинит в двух других файлах, — и он остаётся в двух. Заявленной зоны коммита не нарушает (4 файла как в `--stat`), но **неполон по существу**. Кандидат на follow-up перед upstream: привести `lock_compat_e2e.rs` и `package_e2e.rs` к тому же `path.exists().then_some(path)`.

**F. `pa-edit` в `74d1e3d04`.** Тело коммита перечисляет `pa-edit` среди переформатированных крейтов; `--stat` его не содержит (73 файла: `pa-graph`, `pa-lsp`, `pa-mem`, `pa-types`, `pa-telemetry`, `pa-agent`, `pa-cli/tests`). Расхождение **текста тела**, не содержимого: `pa-edit` уже отформатирован, работы не потребовалось. Также в теле упомянут `needless_sort_by_key`, а в диффе виден только `sort_by_key` (без `Reverse`) и `fetch_update` → `try_update`; фактических `needless_sort_by_key`-правок нет. Обе позиции — косметика описания.

**G. Пре-существующие флапы вне зоны 14 коммитов** (не регрессы — воспроизводятся на родителе `375ad95cf`, файлы не тронуты ни одним из 14 коммитов):
- `pa-core --lib refinement::tests::concurrent_update_harness_state_writes_all_land` — `Lock file is already being held` при параллельном прогоне; изолированно 3/3 зелёный; на родителе 1 ok / 4 FAIL из 5.
- `pa-daemon --lib worker::tests::{kill_broadcast,abort_boundary}_tests::*` — тайминговые; на родителе 1–3 ok из 5 (та же картина на HEAD).
- `pa-core --lib mcp::oauth_callback::tests::error_callback_settles_none` — «ports 53700-53709 are all in use» при параллельном прогоне с другими OAuth-тестами; изолированно 3/3 зелёный.
- Полные прогоны библиотек на HEAD (изолированное окружение): `pa-daemon` 1104 passed / 0 failed; `pa-tui` 1436 / 0; `pa-cli` 309 / 0; `pa-ai` 398 / 0; `pa-graph` 437 / 0; `pa-lsp` 110 / 0; `pa-mem` 117 / 0; `pa-telemetry` 64 / 0; `pa-types` 140 / 0; `pa-jev` 11 / 0. `pa-core` 1333/1335 в объединённом прогоне — 2 флапа выше, оба зелёные изолированно.

---

## Сверка с body коммитов

| коммит | заявлено в body | найдено в диффе | вердикт |
|---|---|---|---|
| `2b3d74842` | `daemon_rejection_headless` e2e + prompt unit | оба теста присутствуют и краснеют без правки | ✅ |
| `2e79330c3` | payment-frame unit + stalled-peer mock + adapter options test | `in_stream_credit_frame_classifies_as_payment_required`, `a_signal_present_request_still_times_out_on_a_stalled_peer`, `every_stream_attempt_carries_a_bounded_request_timeout` — все три есть | ✅ |
| `ffd91a66b` | refusal funnel + pure rule tests, path-recognition unit, stop-candidate extraction against `install-rust.sh` | `the_funnel_refuses_a_non_https_url_before_fetching`, `the_upstream_ts_daemon_socket_is_recognized_by_path_only`, `the_upstream_ts_daemon_is_never_a_stop_candidate_for_a_fork_root`, `the_fork_run_skips_the_upstream_ts_socket_and_probes_the_fork_socket` | ✅ |
| `58394f09a` | `kernel_error_clip` suite (3 теста) + multibyte walkback | ровно 3 теста в `tests/kernel_error_clip.rs` + `truncates_raw_payload_on_multibyte_char_boundaries` | ✅ |
| `734261a93` | `loop_rpc_spawned_headless` e2e (slow rebuild must not starve input) | `a_slow_compaction_rebuild_does_not_starve_input` | ✅ |
| `5461ba80a` | 4 file_processor tests, «three red on the unfixed tree» | 4 теста; мутация даёт ровно **3** красных | ✅ (совпадает дословно) |
| `07364009a` | compaction estimate tests with synthetic PNG headers | `user_images_estimate_by_geometry_not_zero` + 3 сопутствующих | ✅ |
| `0f7547061` | `the_verifier_never_rides_the_authorization_url` + updated TS-shape tests | тест есть, TS-shape тесты обновлены (`state` вместо `verifier`) | ✅ (см. набл. B) |
| `abe2a419b` | write/append neutralization + read-only reopen в `test_repl.py` | ровно 3 теста | ✅ |
| `37e788625` | pin +00:00 на все семь датированных фикстур, expectations unchanged | 7 строк изменено, 14 дат (author+committer × 7) все `+00:00`, ожидания не тронуты | ✅ |
| `9c6ca829d` | wire fixtures, chunking/retry/cooldown/native-only/cache, create_session gate | 23 теста в `pa-core::jev` + 22 в `pa-jev`; `create_session_installs_jev_tool_only_when_enabled` есть | ✅ (см. набл. C) |
| `5122da3d3` | «Red on the unfixed tree for all four files when run under a live session» | подтверждено экспериментом: с родительскими версиями 4 файлов под живым ambient-окружением — 2+2+1+3 = 8 красных; на HEAD — 0 | ✅ |
| `f6328d9e8` | `docs/fork-identity.md` описывает реализованную модель; `AGENTS.md` wins | содержимое сверено: `HOME` pin, `PRIME_AGENT_CODING_AGENT_DIR`, `PRIME_AGENT_DAEMON_SOCKET=${TMPDIR:-/tmp}/allspark-daemon.sock` — совпадает с `scripts/allspark-launcher.sh`; известные отклонения (`allspark-<uid>`, Windows shared pipe) описаны; `check-fork-identity.sh` честно помечен «not yet implemented» и действительно отсутствует | ✅ |
| `74d1e3d04` | rustfmt по pa-graph/lsp/mem/edit/agent/types/telemetry + clippy-фиксы | `pa-edit` в `--stat` отсутствует (набл. F); остальное сходится | ⚠️ расхождение текста тела (косметика) |

---

## ИТОГ: **GREEN**

Все 14 коммитов проходят регресс-тесты на HEAD; каждый ключевой тест доказан красным на откаченной правке. Файлы каждого коммита — в заявленной зоне, лишних файлов нет. Полное дерево: `cargo check --workspace --all-targets` — 0 ошибок / 0 предупреждений (не-vendor), `cargo fmt --all --check` — чисто, `cargo clippy --workspace --all-targets` — только vendor `crossterm`.

Блокеров для upstream-подготовки нет. Перед выкладкой стоит рассмотреть 3 необязательных follow-up:
1. **E** — распространить skip-контракт `PA_TS_BINARY` на `lock_compat_e2e.rs` и `package_e2e.rs` (иначе эти два падают при заданном-но-отсутствующем TS-бинаре — ровно та поломка, которую чинит `5122da3d3`).
2. **B** — добавить тест уровня `login_anthropic`, чтобы закрепить независимый минтинг `state` (сейчас закреплён только провод).
3. **F** — поправить тело `74d1e3d04` (`pa-edit`, `needless_sort_by_key`) перед публикацией, чтобы сообщение коммита совпадало с диффом.
