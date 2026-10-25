# AGENT-ECOSYSTEM-TESTING-2026 — Тема 2: как тестируют свои агентные CLI/TUI конкуренты, и что из этого применимо к нам

**Дата**: 2026-10-24 · **Режим**: read-only исследование (кода не правил, релизов не делал, git-операций не выполнял,
`allspark`/`prime-agent update` не запускал) · **Целевой форк**: `/root/prime-agent` @ `01de64db2`
· **Отчёт**: `.research/AGENT-ECOSYSTEM-TESTING-2026.md`

Метод: read-only чтение исходников через GitHub API / raw.githubusercontent (закреплённые коммиты ниже),
DeepWiki-запросы по кодовой базе (с проверкой каждого названного файла сырым fetch'ем), локальное чтение дерева
`/root/prime-agent` (file:line), websearch 2025-2026. Всё, что названо именем файла, я открыл и прочитал;
выводы, которые я не проверил исполнением, помечены **UNVERIFIED**.

Закреплённые ревизии (проверены `GET /repos/..`):

| Репозиторий | Коммит | Дата коммита | ★ |
|---|---|---|---|
| `xai-org/grok-build` | `2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8` | 2026-09-29 | 27.5k |
| `openai/codex` | `970111f6cb62cc02fe36a82ea7d71b37a2889c97` | 2026-10-23 | 130.8k |
| `can1357/oh-my-pi` | `0b5c27ab24719c86a5986b95ee55f4e75bc1aaa0` | 2026-10-22 | 36.4k |
| `cline/cline` | `40c5d99031fe1128189c5b52e40638d2b7787754` | 2026-10-24 | 70.8k |
| `All-Hands-AI/OpenHands` | `fee10e3d2909ebbf9950578575b1ee011492171e` | 2026-10-24 | 91.7k |
| `SWE-agent/SWE-agent` | `3ea751c087f32b16e039a2233dd6eefecef325d5` | 2026-07-16 | 20.6k |
| `Aider-AI/aider` | `5dc9490bb35f9729ef2c95d00a19ccd30c26339c` | 2026-05-22 | 49.7k |
| `microsoft/tui-test` | `f2760592ed00e90436f8c3bae124dab712eaecf8` | 2026-10-23 | 295 |
| `RooCodeInc/Roo-Code` | `main` (recursive tree fetched 2026-10-24; per-file lines cited, no single-pin SHA — UNVERIFIED drift) | 2026-05-15 | 24.3k |

---

## 0. Что я добавил нового (и чего в ваших отчётах нет)

Прочитал целиком `blackbox-qa-best-practices.md`, `fork-testing-research.md`, `TEST-MATRIX.md`, `oh-my-pi.md`,
`oh-my-pi-deep.md`. `fork-testing-research.md` уже закрывает **grok-build**, **codex** и **oh-my-pi** по линии
«тестовая инфраструктура» (MockInferenceServer, PTY harness, leader cluster, soak, wire pin). Новое здесь:

1. **Шесть репозиториев, которых в отчётах нет вообще**: `cline/cline`, `All-Hands-AI/OpenHands`,
   `SWE-agent`, `Aider`, `microsoft/tui-test` (инструмент, не агент), `RooCodeInc/Roo-Code` — с точными
   файлами и инвариантами.
2. **Класс «данные/конфиг/каталог молча пустые»** — отдельный раздел (§5) с четырьмя промышленными механизмами
   из четырёх репозиториев: `must contain at least one model` (codex), `assertNonEmptyModels` (cline),
   `expect(AUTOMATION_CATALOG.length).toBeGreaterThan(0)` + защита от вакуумного цикла (OpenHands),
   `cacheAuthoritative = ... && dynamicModels.length > 0` (oh-my-pi). Плюс `minItems`-эквивалент в JSON Schema
   и `at_least_one` в dbt как промышленные аналоги инварианта.
3. **Build identity / stale-daemon detection** — целиком новый раздел (§7), это вторая половина вашей задачи
   после уточнения «нужен хэш сборки, а не только версия». Разобрал четыре независимых реализации
   2025-2026: cline (`sdk-v1-<sha256 sources>` + epoch + total order), codex (`sha256("git:<commit>:<target>")`
   + `server_version_notice_kind`), grok-build (`option_env!("VERSION_WITH_COMMIT")` + `leader_is_older_than`
   + `x.ai/leader/version_mismatch` toast), omp (`RuntimeIdentity` + `sameRuntimeIdentity`, `unknown` как
   wildcard). И конкретные file:line в нашем дереве, где сейчас расходятся **два** классификатора демона.
4. **Наше собственное состояние по этим двум темам с file:line** (§8): что уже есть (`doctor`, `classify_reachable`,
   `ensure_daemon_running`+`shutdown_stale_daemon`, vt100 PTY, `bundle_catalog.py` форк-гейт), что сломано прямо
   сейчас (§8.3 — гейт `FORK_PROVIDERS` красный на `--fixture`, я это воспроизвёл), и чего нет
   (build id, non-empty инвариант на data-path рантайма).
5. **Roo Code** как контрапункт «нет TUI → реестр команд как контракт» (§4.5).
6. **Таблица «практика → кто применяет → что даёт → цена»** (§6) с ценами, измеренными локально там, где можно.

Пункты 1-6 не повторяют `fork-testing-research.md` §1-3: я не пересказываю MockInferenceServer,
Conversation::nth, dhat_soak, wire_shape и т.д. Где они нужны для аргумента, я ссылаюсь, а не описываю заново.

---

## 1. grok-build (xAI) — что видно в открытом дереве

Дерево публичное (Apache-2.0), `CONTRIBUTING.md` прямо говорит: внешние PR не принимаются, дерево опубликовано
для прозрачности. То есть это **их собственный** тестовый набор, не «написано для контрибьюторов» — хороший
источник того, что команда считает достаточным.

### 1.1 Масштаб (посчитано по recursive tree)

| Метрика | Значение |
|---|---|
| blobs | 4 487 |
| файлы с `test` в пути | 1 247 |
| `crates/*/tests/` файлы | 567 |
| `xai-grok-pager-pty-harness/tests/pty_e2e/` | **217** |
| `.../tests/leader_pty_e2e/` | 7 |
| декларативные `tests/scenarios/*.yaml` | **45** |
| `xai-grok-test-support/src/` | 72 файла |

### 1.2 Каталог/пикер: как именно проверяется, что модели есть

Это ближе всего к нашему инциденту.

- **`crates/codegen/xai-grok-models/src/lib.rs:12`** — `DEFAULT_MODELS_JSON: &str = include_str!("../default_models.json")`;
  `lib.rs:31` — JSON парсится в `LazyLock`, `lib.rs:37-40` — `assert!(model_ids.contains(&defaults.default))`
  с сообщением `"default_models.json: 'default' is '{}' but 'models' array only has {model_ids:?}"`.
  То есть **dotaz default ∈ models проверяется в рантайме, на каждом старте, из зашитых байт**, а не тестом.
  Это дешёвая реализация «инвариант непустоты»: нет отдельного теста, который можно забыть.
- **`crates/codegen/xai-grok-shell/src/agent/remote_config/resolution.rs:487`** — `validate_selectable()`
  возвращает `Err`, когда allowlist не оставил ни одной selectable-модели, и отдельно проверяет,
  что явно заданный default не исключён политикой (`resolution.rs:505-520`).
- **`resolution.rs:467`** — `models_endpoint_empty_message()`: пустой каталог **имеет своё сообщение**
  («No models are available: {url} returned none or could not be reached. Check the endpoint and your login»),
  а не сливается с generic-ошибкой. Это тот самый fail-loud, которого не хватило нам.
- **`agent/chat_modes.rs:159`** — `fn empty_state() -> acp::SessionModelState` с пустым id и пустым списком;
  и `chat_modes.rs:101` — `tracing::warn!(raw_modes = resp.modes.len(), "chat modes: fetch returned modes but none selectable after availability filter")`.
  Т.е. «получили непустое, но после фильтра пусто» — отдельный, именованный лог-сигнал.
- **`config_tests.rs:7984`** — `resolve_model_list_prefetch_replaces_bundled_entirely`: **явный тест на то, что
  внешний (packaged/prefetch) каталог полностью заменяет bundled**. У нас это поведение `chain.rs:183`
  (`Some(bundled) => bundled`) **не покрыто тестом на «после замены модели остались»** — см. §8.
- **`config_tests.rs:~8009`** — `resolve_model_list_empty_prefetch_yields_empty_base()`: пустой prefetch
  даёт пустую базу. То есть «пусто» — это **допустимое, тестируемое состояние**, и UI обязан его рендерить.
  У них пустота каталога — не баг сама по себе; баг — не сказать об этом.

### 1.3 PTY e2e: 217 императивных + 45 декларативных сценариев

- Декларативный раннер: `crates/codegen/xai-grok-pager-pty-harness/src/scripted.rs` (85 KB).
  `ScriptedScenario { name, description, terminal, environment, workspace, mock, steps }`.
  `scripted.rs:75` — `fn validate()` отказывает сценарию без шагов («scenario {} has no steps»);
  это ровно инвариант «тест не может быть пустым» на уровне тестового фреймворка.
- Шаги (`scenario step enum` (`scripted.rs:204`, `pub enum ScenarioStep`)): `WaitForText`, `AssertContains`, `AssertNotContains`,
  `TypeText`, `Keys`, `Paste`, `MouseClick`, `Resize`, `Screenshot`, `AssertRunning`, **`AssertRequestContains`**
  (проверка тела запроса к мок-инференсу!), `AssertRequestImageCount`, `AssertInlineImages`,
  `AssertNoTempArtifacts`. Последние четыре — это assert на **провод**, а не на экран; комбинация
  «экран + провод» в одном сценарии — то, чего у нас в PTY-тестах нет (у нас faux-скрипт, а не мок-сервер
  с логом запросов — см. `fork-testing-research.md` §1).
- Конкретный пример, близкий к нашему дефекту: `tests/scenarios/dashboard_model_list_click.yaml` —
  `type_text: "/model "`, `wait_for_text: test-model`, `click_text: test-model`,
  `assert_contains: test-model`, `assert_not_contains: panicked`, `screenshot`.
  Т.е. **путь пользователя «открыть пикер → увидеть модель → выбрать» закреплён сценарием**, а мок-сервер
  экспонирует `test-model` из `MockModelEntry` (`xai-grok-test-support/src/mock_server.rs:49` `pub struct MockModelEntry`,
  `:63` `impl MockModelEntry`; `:350` `pub fn install_models`).
- `tests/scenarios/auto_compact_resize.yaml` — тот же приём для «порог компакции»: три `resize` + assert
  на исчезновение/появление sticky-заголовка. Урок: **пороговое поведение проверяется сменой размера
  терминала, а не юнитом на threshold-константу.**

### 1.4 Stale binary / build identity (ключ к нашей второй половине задачи)

Grok-build — единственный из четырёх, кто **сравнивает версию клиента и сервера и говорит об этом пользователю**:

- `crates/codegen/xai-grok-pager-bin/build.rs:30` — `cargo:rustc-env=VERSION_WITH_COMMIT={version} ({commit})`,
  commit = `git rev-parse HEAD` первые 12 символов, `build.rs:13-22` — `cargo:rerun-if-changed` на
  `HEAD`, `logs/HEAD` и текущую ветку (иначе cargo не пересобирает при коммите).
- `crates/codegen/xai-grok-version/src/lib.rs:11` — `VERSION = match option_env!("GROK_VERSION")` (иначе `CARGO_PKG_VERSION`);
  `lib.rs:17` — `IS_DEV_BUILD`;
  `lib.rs:25` — `set_full_version()`, `lib.rs:30` — `full_version()`; `pager-bin/src/main.rs:2037` —
  `xai_grok_version::set_full_version(env!("VERSION_WITH_COMMIT"))` в первой строке `main`.
  Комментарий `lib.rs:20-22` объясняет архитектурное решение: **stamp живёт в финальном бинаре,
  чтобы git-операции не инвалидировали весь граф библиотек**. Это прямой ответ на вопрос «где ставить build-id».
- `crates/codegen/xai-grok-shell/src/leader/mod.rs:97` — `const CLIENT_LEADER_VERSION: &str = xai_grok_version::VERSION;`
  `mod.rs:105` — `pub fn leader_is_older_than(leader_version, baseline) -> bool` (semver, unparseable → `false`,
  т.е. «неизвестное не понижаем»); `mod.rs:117` — `fn should_evict(leader_version, client_version)`;
  `mod.rs:109-116` — «a newer client replaces an older leader, never the reverse, so the machine converges
  to the newest version without thrash».
- `crates/codegen/xai-grok-shell/src/leader/server.rs:1479` — `fn make_version_mismatch_notification(client_version, leader_version)`
  строит `x.ai/leader/version_mismatch` c `clientVersion`/`leaderVersion`; равенство или `"unknown"`
  → `None` (не шумим).
- `crates/codegen/xai-grok-pager/src/acp/version_mismatch.rs:24` — декодер `version_mismatch_banner`; текст
  `"⚠ Version mismatch: client {client}, leader {leader}. Restart grok to match"`;
  `acp/version_mismatch.rs:28` — версии санитизируются, и значение из одних пробельных/control символов
  отбрасывается (`if client.chars().all(char::is_whitespace) ... return None`).
- Тесты: `leader/server_tests.rs` — `decide_relaunch_is_idempotent_and_directional`,
  `version_mismatch_notification_contains_correct_fields`,
  `version_mismatch_notification_is_none_when_versions_match`,
  `version_mismatch_notification_is_none_for_unknown_leader_version`;
  `app/acp_handler/tests/version_mismatch.rs` — то, что баннер **не попадает в scrollback** и
  **переживает** reconnect-тосты (т.е. пользователь не теряет сигнал).
- `crates/codegen/xai-grok-shell/src/leader/server.rs:1405` — `decide_relaunch_for_update` (directional guard:
  `leader_is_older_than(leader, to_version)` только тогда relaunch), `server.rs:1441` — `spawn_relaunch_drain`
  (bounded grace, flush всех сессий, `ShutdownReason::AutoUpdate`).

Что это значит для нас: у grok-build **версия — это идентификатор сборки**, потому что они собирают одну
версию один раз в релизном конвейере; «пересобрать ту же версию с другим кодом» у них не поддерживается
как штатный сценарий (никто не собирает из исходников и не поднимает демон рядом с релизным).
У нас это **штатный** сценарий (форк + локальные dist-сборки), поэтому одной версии нам мало — см. §7.

### 1.5 Чего у grok-build НЕТ

- Нет content-hash сборки. `VERSION_WITH_COMMIT` = версия + 12 hex git-коммита; две сборки одного коммита
  с разными feature-флагами неотличимы (**UNVERIFIED**, что у них вообще есть feature-флаги, влияющие на
  поведение демона).
- Нет `--version` инварианта «строки, разбираемой установщиком» в том виде, как у нас (`install-rust.sh`
  парсит второй токен; см. §7.6).

---

## 2. openai/codex — эталон инварианта «каталог не может быть пустым»

`fork-testing-research.md` §1 уже подробно разобрал их harness (wiremock, `context_snapshot.rs`,
`StreamingSseServer`, `retry_after.rs`, `mcp_conformance`). Здесь **только то, чего там нет**: модель-каталог
как контракт и build identity.

### 2.1 Non-empty invariant: три уровня, все явные

| Уровень | Файл:строка | Что именно |
|---|---|---|
| Пользовательский файл каталога | `codex-rs/core/src/config/mod.rs:2223` `fn load_catalog_json`, `:2234` `if catalog.models.is_empty()`, `:2238` `"model_catalog_json path `{}` must contain at least one model"` | Загрузка `--model-catalog-json` **отвергает пустой каталог** как `ErrorKind::InvalidData` |
| Тест на предыдущий пункт | `codex-rs/core/src/config/config_tests.rs:~1204` `model_catalog_json_rejects_empty_catalog` | пишет `{"models":[]}` → `expect_err`, проверяет `InvalidData` + фразу |
| Рантайм-старт | `codex-rs/core/tests/suite/remote_models.rs:1490` `empty_model_catalog_requires_explicit_model` | пустой/недоступный каталог → **отказ стартовать**: `"No models are available. Set `model` explicitly or check your model catalog configuration."` (два `test_case`: без модели и с явной моделью) |
| Слияние remote+ bundled | `remote_models.rs:1204` `remote_models_merge_preserves_bundled_models_on_empty_response` | remote вернул `[]` → bundled-модель **всё ещё доступна** |
| Реализация политики | `codex-rs/models-manager/src/manager.rs:640` `// A failed fetch is not an authoritative empty catalog. Keep the last successful snapshot` | различает «пусто потому что сбой» и «пусто потому что правда пусто» |
| «Пусто вместо мусора» | `manager.rs:538` `Cow::Owned(self.catalog_source.fallback_models().unwrap_or_default())` + `manager.rs:751` merge | bundled — base, remote — override/append |

Ключевая мысль, которую стоит перенести буквально: **пустой каталог — это отдельное наблюдаемое состояние
с собственным сообщением об ошибке, а не «просто пустой список»**. Наш инцидент — ровно противоположное:
`chain.rs:183` вернул bundled, в худшем случае пустой после pinning, и никто не сказал «каталог пуст».

### 2.2 Build identity: `sha256("git:<commit>:<target>")`

- `codex-rs/build-info/src/lib.rs:21` — `macro_rules! initialize` → `option_env!("STABLE_GIT_COMMIT").unwrap_or("dev")`.
  Комментарий `lib.rs:16-20`: «The environment lookup intentionally expands at the macro call site so Git
  changes invalidate only final binary actions, not this shared library. Cargo builds must supply
  STABLE_GIT_COMMIT in the build environment.» — это ровно проблема, которую `build.rs` grok-build решает
  через `rerun-if-changed=HEAD`; codex решает через **переменную окружения на финальном крейте**.
- `lib.rs:131` — `pub fn build_id(build_commit: &str, target: &str) -> Option<String>`:
  требует ровно 40 hex, считает `Sha256::digest(format!("git:{commit_lower}:{target}"))`, возвращает
  `Some("sha256:<hex>")`. Док `lib.rs:128-130`: «This identifies a standard build configuration,
  **not exact executable bytes**» — честно про границу применимости.
- `lib.rs:108` — `fn resolve(install_context, build_commit)`: версия берётся из `package.json`-манифеста рядом
  с бинарём (`codex-package.json`), commit — из stamp'а. То есть **версия и build-id разведены**:
  версия может прийти из манифеста (переклеена при упаковке), build-id — только из компиляции.
- `codex-rs/build-info/build.rs:5` — `cargo:rustc-env=CODEX_BUILD_TARGET={TARGET}`; target входит в id,
  поэтому x86_64 и aarch64 сборки одного коммита **различны** (правильно для нас: у нас тоже 2 архитектуры).
- `codex-rs/exec-server/tests/initialize.rs:26-40` — тест **явно** выставляет `STABLE_GIT_COMMIT`,
  `GITHUB_SHA`, `CODEX_BUILD_TARGET` в окружение дочернего процесса и утверждает
  (`initialize.rs:79`) `expected_environment.provider_id = build_id(TEST_BUILD_COMMIT, target)`:
  «Runtime environment variables cannot replace the executable's build stamp». Это готовый шаблон
  теста на наш §7.4: **runtime env не должен подменять build id**.
- `codex-rs/tui/src/update_versions.rs:30` — `pub(crate) fn server_version_notice_kind(client, server) ->
  Option<ServerVersionNoticeKind{Older,Different}>`: локальная сборка (`0.0.0`) или другой release line →
  `Different`; стабильный клиент и сервер старее → `Older`; иначе `None`. Таблица кейсов в тестах
  (`update_versions.rs:~95-140`) включает `("0.153.0","0.152.0+dev") -> None` — т.е. **+build suffix
  не считается «старее»**.
- `codex-rs/tui/src/app/server_version_notice.rs:7` — `refresh_server_version_overview_notice` → строка
  `"Service v{server} {<|≠} Codex CLI v{client}"` (snapshot-тест:
  `tui/src/history_cell/snapshots/codex_tui__history_cell__tests__connected_server_version_notice_snapshot.snap`).

### 2.3 Чему из codex НЕ стоит подражать

Требование «нет каталога → не стартуем» для нас **слишком строго**: у нас no-cold-start chain — продуктовая
фича (см. `chain.rs:1-6`), и отказ стартовать при пустом bundled сломал бы офлайн-первый запуск.
Нам нужен средний вариант: стартовать на compiled-fallback, но **громко** сказать, что bundled был
отброшен (см. §8.4, пункт P1).

---

## 3. cline/cline — самый близкий к нам случай: build identity как первоклассный объект

cline — единственный, у кого **есть и content-hash сборки, и total order версий, и детекция stale-демона
по хэшу**. Это прямой образец для §7.

### 3.1 Как строится build id

`sdk/packages/core/scripts/runtime-build-id.ts`:

- `:41` `export function resolveSdkRuntimeBuildId(repoRoot: string): string`
- `:19-38` `collectRuntimeSourceFiles` — рекурсивный обход `src/` пакетов `shared, llms, agents, core, sdk`,
  **исключая** `__tests__ / fixtures / tests` и `*.test.*` / `*.spec.*`. Т.е. хэш считается по **коду,
  который может исполняться в демоне**, а тесты в него не входят.
- `:44-52` inputs = `bun.lock` + `core/build-plugin-runtime.mts` + `package.json` каждого пакета +
  (опционально) `bun.mts` + все runtime-исходники.
- `:58-64` — `createHash("sha256")`, по каждому файлу `hash.update(relative_path); hash.update("\0");
  hash.update(bytes); hash.update("\0")`, результат `` `sdk-v1-${hex}` ``.
- Тест `sdk/packages/core/src/hub/discovery/runtime-build-id.test.ts` — четыре кейса:
  «deterministic and changes with runtime source», «changes with plugin runtime packaging»,
  «changes when the shipped SDK facade changes», **«ignores test-only source changes»**.
  Это ровно та четвёрка свойств, которую надо утверждать для любого build-id (см. §7.3).

### 3.2 Как это применяется в рантайме

`sdk/packages/core/src/hub/discovery/index.ts`:

- `:122` `resolveHubBuildId()`: env `CLINE_HUB_BUILD_ID` → embedded `__CLINE_CORE_RUNTIME_BUILD_ID__`
  → `source-<version>`. env-override существует **для тестов** (и это задокументировано в `:119-121`).
- `:140` `resolveHubBuildEpochMs()`: то же для timestamp (ordering).
- `:168` `getManagedHubCompatibility(record, expectedBuildId)`: сначала `isHubProtocolCompatible`,
  затем `buildId` пуст → `reason: "missing_build"`, не совпал → **`reason: "build_mismatch"`** (`:181`).
- `:244` `compareHubBuilds(a, b)`: total order — одинаковый id → 0; затем `buildEpochMs`; затем release-версия;
  затем id как детерминированный tiebreak. Комментарий `:300-305`: «Because the decision is derived from a
  total order, **two installations can never retire each other**». Это самая ценная идея всего раздела:
  без total order два процесса с разными сборками будут каждый считать себя новее и убивать друг друга.
- `:307` `isManagedHubReusable(record, {self})`: совместим по build id → reuse; `build_mismatch`/
  `missing_build` → reuse **только если** `compareHubBuilds(self, record) <= 0`; protocol-incompatible → replace.
- Док `:160-168`: «reusing a daemon from another build would keep executing stale runtime, scheduler,
  connector, and command-handler code after an upgrade» — это буквально наш инцидент, только у нас
  stale-демон исполняет старый код **и никто этого не замечает**.
- `managed-hub-build-watcher.test.ts` — «watcher prompts the user to update», т.е. **сборки разные → не убивать
  сразу, а сказать пользователю**.

### 3.3 Каталог/провайдеры

- `sdk/packages/core/src/services/llms/configured-provider-registry.ts:47` —
  `function assertNonEmptyModels(providerId, models)`: `throw new Error('Provider "{}" must include at least one model.')`.
  Вызывается из `register()` (`:107`), и там же `register()` проверяет, что `defaultModel` определён и входит
  в список.
- Тест `sdk/packages/core/src/services/providers/local-provider-service.test.ts` — «provider must have at
  least one model when being updated», «at least one model is required» (**UNVERIFIED** в деталях: файл
  не открывал, назван DeepWiki; саму функцию `assertNonEmptyModels` я прочитал).
- `evals/smoke-tests/` — 8 сценариев (`01-create-file` … `08-openai-compat-gpt-oss-edit`), каждый —
  `scenarios/<name>/config.json` с `{name, description, prompt, expectedFiles, expectedContent, timeout}`
  (`evals/smoke-tests/README.md:44-60`, пример `scenarios/01-create-file/config.json`).
  Оракул — **`expectedFiles` + `expectedContent.contains`**, то есть состояние диска, не текст ответа.
  Раннер `run-smoke-tests.ts` (`:100-101` — поля оракула, `:14` — `--trials`, `:618` — pass@k)
  считает `pass@k` / `pass^k` через `MetricsCalculator` (`evals/analysis/src/metrics.ts:27`
  `passAtK`, `:60` `passCaretK`, `:96` `flakinessScore` как бинарная энтропия).
- `evals/analysis/patterns/cline-failures.yaml` — **таксономия провалов** по regex + `category`
  (`provider_bug | transient | harness | environment | policy | auth`) + `issue:` ссылка. Это дешёвый
  способ превратить «красный eval» в «класс провала» без LLM-судьи.
- `evals/ARCHITECTURE.md` — трёхуровневая пирамида: Layer 1 contract tests (no LLM, fast),
  Layer 2 smoke (5-8 сценариев, 3 модели, pass@k), Layer 3 e2e cline-bench (nightly).
  Важно: в том же файле честно написано, что Layer 2 **сейчас частично отключён** («Smoke tests
  are partially disabled while the eval framework is repointed at the new SDK CLI»). То есть даже у
  зрелого проекта «smoke на каждую фичу» деградирует, когда меняется раннер — аргумент за то,
  чтобы наш гейт жил в **уже работающем** раннере (`scripts/qa/run_suite.py`), а не в отдельном.

### 3.4 doctor

`apps/cli/src/commands/doctor.ts` — ищет stale background PID по startup-artifacts (`readStartupArtifact`,
`listHubStartupLocks`, `isProcessRunning`). Это та же ниша, что наш `allspark doctor`, но у cline
**doctor читается рантаймом** (`isManagedHubReusable`), а не только человеком — см. §8.2.

---

## 4. OpenHands, SWE-agent, Aider, tui-test — публичные практики

### 4.1 OpenHands (TS, agent canvas + agent-server)

Дерево: 2 782 blobs, 972 «test-ish», 54 Playwright-спеки в `tests/e2e/`.

- **`docs/TESTING_MATRIX.md`** — та самая «smoke на каждую пользовательскую фичу»: матрица
  `Install (npm|Docker) × OS × Agent` и таблица фич (Onboarding, Conversation, Terminal tool, File editor,
  Browser tool, LLM profiles, Secrets, Automations, Files/diff, MCP install, Image upload, Key rotation)
  с P0/P1/P2 и **явным списком непокрытого** («Not yet covered by CI: real ACP credentials, macOS,
  public auth mode, subscription login paths, Windows»). Это лучший из увиденных артефактов
  «что мы НЕ тестируем» — и он в репозитории, а не в голове.
- **`tests/e2e/mock-llm/`** — Playwright против **настоящего стека** (`bin/agent-canvas.mjs`, тот же,
  что `npx @openhands/agent-canvas`) с **мок-LLM сервером** `scripts/mock-llm-server.py`: admin-endpoints
  `POST /admin/reset`, `/admin/trajectory/register`, `/admin/trajectory/activate`, `GET /admin/requests`.
  Спект-файлы разложены по фичам (`settings/`, `conversations/`, `files/`, `automations/`, `onboarding/`,
  `backends/`, `home/`, `mcp/`, `skills/`, `canvas-extensions/`, `regressions/`) — ровно «каталог = реестр
  пользовательских поверхностей».
- **`.agents/skills/e2e-testing/references/guide.md`** — операционный документ на ~20 KB: изоляция state
  (`.tmp/mock-llm-state`), per-run session key, «не поллить аналитику» (`guardAgainstPostHogRequests`),
  запрет трейсов на live-тестах из-за утечки ключей, отдельный live-раннер (`npm run test:e2e:live`,
  `--check` для валидации окружения). Ключевая фраза: *«Live LLM-backed tests must never run as part of
  `npm run test:e2e`»* — у нас то же разделение (`--lane real-model` vs faux/mock-provider, см.
  `qa-tasks/README.md`), так что тут мы уже соответствуем.
- **Non-empty инвариант** (прямо в тему §5):
  `__tests__/constants/extensions-catalogs.test.ts:9` `expect(AUTOMATION_CATALOG.length).toBeGreaterThan(0)`,
  и — самое ценное — `:19` комментарий: *«Declaring none is legitimate … so the resolution above is only
  worth asserting while some entry still declares one. **Without this the loop above would pass over an
  empty catalog.**»* Это защита от **вакуумного** теста: цикл `for (automation of CATALOG)` на пустом
  массиве зелёный. Такой класс тестов у нас есть (см. §8.3: `test_fixture_passes_the_full_gates`
  зелёный, а `verify` красный — вакуум по построению).
- `:36` — ещё инвариант: `expect(SETUP_REGISTRY.entries.map(e => e.id)).toEqual(shipped.map(a => a.id))`,
  с комментарием «a shipped one that fails admission is **dropped silently**» — т.е. они тестом
  фиксируют «всё, что корабль отгрузил, прошло admission», а не «admission работает на одном примере».
  Наш аналог — `bundle_catalog.validate_bundled_catalog_dir`, но он не сравнивает «что реально в
  артефакте» с «что читает рантайм» (см. §8.3).
- `__tests__/manifests/manifest-validation.test.ts` — таблица негативных кейсов (`it.each`) с комментарием
  `:113` *«Each case is a separate invariant the host enforces on data authored in another repository»*:
  version не semver, markup в copy, неизвестный placeholder namespace, cap на длину, дубликаты полей.
  Это образец «schema-валидация артефактов» из вашего п.4.
- `__tests__/api/mock-settings-catalog.test.ts` — `/server_info` и `/api/options/models` проверяются как
  контракт (default_model ∈ models, `verified_providers` непусты и уникальны). Это “каталог как контракт”.

### 4.2 SWE-agent (Python)

- 409 blobs, 108 test-ish, `tests/test_data/` — 56 фикстур.
- Стиль: **PredeterminedTestModel** (детерминированная модель в тестах):
  `tests/test_agent.py:~60` — `test_agent.model = PredeterminedTestModel(["raise_cost"])`, и проверка
  `r.info["exit_status"] == "exit_cost"`. То есть **исход исполнения — часть контракта раннера**,
  проверяется перечислением (`exit_cost`, `exit_context`, `exit_environment_error`, `exit_format`).
  У нас эквивалент — `outcome` в `qa-tasks/task.yaml`, но не как enum-перечисление исходов.
- `tests/test_run_replay.py` — **replay записанной траектории** (`RunReplayConfig(traj_path=...)`,
  `_require_zero_exit_code=True`). Replay — самый дешёвый дифференциальный тест: одна и та же траектория
  должна проигрываться на новой версии. У нас есть golden-replay против TS (`golden_replay.rs`), но
  **не replay прод-сессий собственного продукта**.
- `tests/test_packaging.py` — 4 строки: `assert __version__.count(".") == 2`. Дешёвый shape-гейт
  версии. Слабее нашего `QA RT-013` (у нас как раз есть).

### 4.3 Aider (Python)

- 691 blobs, 97 test-ish, `tests/fixtures/languages/` — по одному файлу на 40+ языков (для repomap).
- `tests/basic/test_models.py` — 22 KB. Ключевое для темы «каталог как контракт»:
  - `test_model_aliases` — жёсткий список алиасов (`"4o" → "gpt-4o"`, `"sonnet" → "claude-sonnet-4-6"`);
    это pin каталога алиасов, ровно как наш `models_fork.rs` но как **тест**, а не как данные.
  - `test_get_model_info_nonexistent` — `manager.get_model_info("non-existent-model") == {}`:
    **пустой info — легальное состояние**; `aider/models.py:sanity_check_model` тогда печатает warning
    «Unknown context window size and costs, using sane defaults» + fuzzy-подсказки (`print_matching_models`).
    Т.е. неизвестная модель = предупреждение, а не отказ. Это ровно наша ситуация с `Estimated`.
  - `test_sanity_check_models_bogus_editor` — «bogus editor model» → `tool_warning` вызван и содержит
    имя модели. Т.е. **сломанная ссылка на модель обязана назвать модель**.
- `pytest.ini` / `tests/basic/*` — нет PTY/TUI тестов: aider тестирует свой IO через mock-`io`
  (`mock_io.tool_output.assert_called()`). Для CLI-агента без TUI это норма, но для нас это
  **anti-pattern**: их подход не поймал бы «пикер пуст».

### 4.4 microsoft/tui-test (не агент, а инструмент)

Стоит упомянуть, потому что это ровно то, что мы могли бы использовать вместо своего PTY-харнесса:

- 331 blob, 192 test-ish, Rust + Python/JS/Go bindings через FFI (`crates/tui-test-ffi/`).
- API: `Session::new/open/execute/get_by_text(..).last().expect()/close`
  (`README.md:~150` Rust example), CLI `tui-test run/expect text/click text/screenshot`
  (`README.md:~90`), локаторы с `--regex`, `--full` (scrollback), `--whitespace exact|normalize`,
  `--after-text/--before-text`; `--failure-artifacts DIR` + `--failure-artifact-recording` (автоматический
  cast через границу падения), `--diagnostic-context KEY=VALUE`.
- У них есть **явная поддержка «агенты как потребители»** («It works for AI agents that need structured
  access to terminal state»). Для нас это вариант «взять готовый инструмент» вместо 200 строк
  `openpty` в каждом e2e — но это добавит внешнюю зависимость в тестовый слой; цена в §6.

### 4.5 Roo Code (VS Code extension, 3 022 blobs, 629 test-ish)

Не CLI/TUI, но полезен как контрапункт: **там, где нет TUI, «smoke на фичу» превращается в
«список зарегистрированных команд»**. (Дерево `main` на 2026-10-24; отдельного закреплённого SHA
для этого раздела нет — **UNVERIFIED** дрейф строк.)

- `apps/vscode-e2e/src/suite/extension.test.ts:9` — `test("Commands should be registered")` с **литеральным
  списком 22 команд** (`SidebarProvider.open`, `plusButtonClicked`, `newTask`, `acceptInput`, …) и циклом
  `assert.ok(commands.has('roo-cline.'+cmd))`. Это ровно «реестр как контракт» из вашего п.4 —
  самая дешёвая возможная форма: одна проверка ловит удаление/переименование пользовательской команды.
  Конфиг: `apps/vscode-e2e/.vscode-test.mjs` — `mocha` tdd, `timeout: 60000`, `--disable-extensions`.
- 3 других suite-файла (`modes.test.ts`, `subtasks.test.ts`, `markdown-lists.test.ts`) — поведенческие,
  т.е. e2e-слой тонкий, а основной объём (629 файлов) — юниты в `src/core` (159), `src/services` (127),
  `src/api` (81), `cli` (35).
- Урок для нас: **список зарегистрированных команд/тулов как тест** — у нас есть
  `crates/pa-core/tests/prompt_guards.rs` (`selected_tools: Some(vec!["python_repl"])`), т.е. мы уже
  делаем это для тулов модели, но не для CLI-команд (`command_registry::format_top_level_help`).
  Один тест «каждая публичная команда из `public_command.rs:172` есть в help» — 15 строк.

### 4.6 Общее в зрелых проектах (свод)

1. **Мок-сервер с логом запросов** — у всех четырёх (grok `MockInferenceServer` + `RequestLog`,
   codex `wiremock`+`context_snapshot`, OpenHands `mock-llm-server.py` + `/admin/requests`,
   omp `createMockModel` + `mock.calls`). Мы: faux-скрипт **без** request-log (см. §8.4 P0-1).
2. **Декларативный сценарий на пользовательскую поверхность** — grok 45 YAML, OpenHands 54 spec,
   cline 8 smoke-configs. У нас: 17 QA-тасков, из них **0** про «пикер/каталог/версию демона»
   (RT-013 проверяет только stamp строки).
3. **Оракул на состоянии/проводе, не на тексте** — единодушно (см. `blackbox-qa-best-practices.md` §1.3;
   повторять не буду, только добавлю, что OpenHands и cline это подтверждают: `expectedFiles`/`expectedContent`,
   `AssertRequestContains`).
4. **Список непокрытого в репозитории** — только OpenHands (`docs/TESTING_MATRIX.md` «Not yet covered»).
   У нас такого артефакта нет; `qa-tasks/README.md` перечисляет известные red-таски, но не «чего нет вообще».
5. **Build/stale-сборка как тестируемая сущность** — только cline (полноценно: content hash + order)
   и grok-build (версия + commit). codex — версия + build_id в handshake.
   У нас — только версия, и только в `doctor` (§8.2): hello уже несёт `runtime.buildId`, но он равен
   версии (`clients.rs:126`), и TUI-путь его вообще не читает.

---

## 5. Класс ошибок «данные/конфиг/каталог молча пустые»

Это центр нашего инцидента, поэтому разбираю отдельно. Промышленные ответы, все найдены в открытом коде:

### 5.1 Три формы защиты

| Форма | Где | Как выглядит | Ловит |
|---|---|---|---|
| **Fail-closed на границе данных** | codex `core/src/config/mod.rs:2234-2240`; наш `crates/pa-models/src/schema.rs:94` | «пусто → `Err`» + сообщение с путём | пустой/битый вход отвергается с адресом |
| **Non-empty assertion на единицу данных** | cline `configured-provider-registry.ts:47`; OpenHands `extensions-catalogs.test.ts:9` | «провайдер без моделей → throw» | провайдер, который попал в реестр пустым |
| **Non-empty, но с явным исключением пустого состояния** | grok `resolution.rs:467` + `chat_modes.rs:159`; oh-my-pi `model-manager.ts:380` | пусто — легально, но **именованный лог + своё сообщение** | «пусто, а пользователь не знает почему» |

Третья форма — правильная для нас: наш продукт обязан работать на compiled-fallback
(`chain.rs:165-190`), значит «пусто» нельзя превращать в отказ; но нужно (а) отличить «пусто, потому что
всё отброшено pinning'ом» от «пусто, потому что слой не сработал», и (б) сказать это в UI/логе.

### 5.2 Отдельно: инвариант «провайдер, показанный в UI, обязан иметь модели»

Наш UI показывает провайдеров отдельно от моделей — это и создало симптом «провайдеры есть, моделей нет».
Три репозитория решают именно это:

- cline: `assertNonEmptyModels(providerId, models)` вызывается **в `register()`**, то есть провайдер
  физически не может попасть в реестр пустым (`configured-provider-registry.ts:94-100`).
- OpenHands: `expect(AUTOMATION_CATALOG.length).toBeGreaterThan(0)` + «иначе цикл пройдёт по пустому
  каталогу» (`extensions-catalogs.test.ts:9,19`).
- grok: `MockModelEntry` в моке + `dashboard_model_list_click.yaml` проверяет «провайдер → пикер → модель»
  через **экран**, т.е. инвариант закреплён на уровне пользовательского пути, а не структуры.

У нас: `crates/pa-tui/src/provider_auth.rs:202` держит `providers: Vec<ProviderRow>` — это список
**провайдеров для логина**, и он строится из auth-конфига, не из каталога моделей. Модели — отдельно
(`model_picker`), и `an_empty_catalog_opens_the_empty_panel`
(`crates/pa-tui/src/model_picker/tests.rs:579`) **утверждает, что пустой каталог рендерится как
«No matching models»**. То есть на уровне TUI пустота — ожидаемое состояние, и это правильно;
но **никто не проверяет, что провайдер из auth-списка имеет хотя бы одну модель в каталоге** — ни тест,
ни рантайм. Это и есть место, где родился симптом.

### 5.3 Чем это отличается от «просто теста на непустоту»

Вакуумный тест: `for (row of catalog) assert(invariant(row))` — на пустом каталоге зелёный.
OpenHands закрывает это отдельным `assert` на непустоту **перед** циклом. У нас есть структурный
эквивалент проблемы: `scripts/release/test_catalog_assets.py:202` `test_fixture_passes_the_full_gates`
проверяет «fixture проходит FULL gates», но `--fixture` **по построению** получает
`allow_small_fixture=True` (`bundle_catalog.py:842`), поэтому часть гейтов не исполняется —
а на `verify` (без waiver) тот же артефакт падает (я воспроизвёл, §8.3). Это тот же класс:
**тест проверял не тот режим, в котором продукт работает**.

### 5.4 Промышленные аналоги инварианта (для цитирования в PR)

- JSON Schema: `minItems` — единственный способ выразить «массив непустой»; `required` этого не делает
  (json-schema.org, «items»: «This keyword does not prevent an array instance from being empty. If needed,
  use `minItems`»).
- dbt `at_least_one` (elementary-data) и `expect_table_row_count_to_be_between` (Great Expectations) —
  тот же инвариант в data-quality: «если строк 0, это провал, а не успех».
- Prometheus `absent()` / `absent_over_time()` — «метрика исчезла» как отдельный алерт
  (dead-man's switch). Прямой аналог для нас: «каталог моделей не пришёл» должно быть сигналом,
  а не тишиной.
- Fail-closed vs fail-open: authzed.com/blog/fail-open — общий словарь; для нас вывод: **на границе
  доверия (что пойдёт в промпт/на провод) — fail-closed; в пути «показать пользователю» — fail-open
  с громким сигналом.**

---

## 6. Таблица: практика → кто применяет → что даёт → цена

Цены — либо измерены локально, либо оценены по числу файлов/строк; «UNVERIFIED» где не мерил.

| # | Практика | Кто применяет | Что даёт нам | Цена |
|---|---|---|---|---|
| 1 | **Build id = sha256 по содержимому сборки**, в hello/`--version` | cline (`sdk-v1-<sha256>`, `runtime-build-id.ts:41`), codex (`sha256("git:c:t")`, `build-info/lib.rs:131`) | отличает «ту же версию, другой код» — наш слепой случай | `build.rs` в composition root (1 файл ~60 строк) + поле в hello + 3 теста. ~0.3 с к общей сборке (замерено: `PROBE_STAMP=A/B` меняет вывод, `.tmp`-проба) |
| 2 | **Total order сборок** перед «убить stale-демон» | cline (`compareHubBuilds`, `hub/discovery/index.ts:244`) | два процесса не убивают друг друга | ~40 строк + property-тест (антисимметрия/транзитивность). Дёшево |
| 3 | **`missing_build`/`build_mismatch` как отдельные причины отказа** | cline (`:168-183`) | «старый демон» перестаёт быть невидимым | один enum + match. Дёшево |
| 4 | **Один классификатор «свой/чужой» на всех путях** | — (у всех по одному) | у нас их **два**, расходятся (§8.2) | удаление одного ~30 строк; тест, что оба пути дают одну классификацию |
| 5 | **Non-empty инвариант на каждый data-path** (`assert` в конструкторе, не в тесте) | cline `assertNonEmptyModels`; grok `models/lib.rs:37`; codex `config/mod.rs:2234` | «провайдер с пустым списком» невозможен по построению | ~10 строк в `load_built_in_models` + 1 unit-тест на каждый data-path |
| 6 | **Пустой каталог = отдельное сообщение/лог** | grok `models_endpoint_empty_message`; oh-my-pi `warn` при «modes есть, selectable нет» | пользователь видит причину, а не «No matching models» | ~15 строк + 1 test |
| 7 | **Защита от вакуумного теста** (`assert non-empty` перед циклом) | OpenHands `extensions-catalogs.test.ts:9,19` | тест реально что-то проверяет | ~3 строки на тест; окупается сразу (наш `test_fixture_passes_the_full_gates` — живой пример) |
| 8 | **Мок-инференс с request-log** | grok `RequestLog`, OpenHands `/admin/requests` | assert на провод + экран в одном сценарии | у нас faux; провод-мок = большой кусок (`fork-testing-research.md` P0-1). Средняя цена, высокая отдача |
| 9 | **Декларативный PTY-сценарий на каждую поверхность** | grok 45 YAML + 217 .rs | «пикер показал модель» становится регрессией | у нас есть vt100+openpty (`pa-perf/src/bench/pty.rs`, 4 e2e); нужен формат + 5 сценариев. Средняя |
| 10 | **`--version` и манифест артефакта сверяются независимо** | наш `verify_release.py:216`, `install-rust.sh:2820`, codex `exec-server/tests/initialize.rs:79` | опубликованный бинарь = проверенный бинарь | у нас уже есть; нужно добавить build-id в оба места |
| 11 | **Список непокрытого в репозитории** | OpenHands `docs/TESTING_MATRIX.md` | не изобретаем заново | ~1 файл, обновление при фиче |
| 12 | **Таксономия провалов** (regex → класс) | cline `evals/analysis/patterns/cline-failures.yaml` | «красный eval» → «класс дефекта» без LLM | ~1 файл + 20 строк кода в раннере |
| 13 | **Replay записанной траектории** | SWE-agent `tests/test_run_replay.py` | дифференциальный тест без модели | средняя (нужен формат записи) |
| 14 | **PTY-инструмент вместо своего харнесса** | microsoft/tui-test | локаторы/скриншоты/артефакты бесплатно | внешняя dev-зависимость; наш harness уже работает |
| 15 | **`Pass^k`/flakiness в гейте** | cline `metrics.ts:27,59,94`; наш `qa-tasks/README.md` уже считает pass-rate | различает «модель не умеет» и «модель нестабильна» | уже есть |

---

## 7. Build identity: как это делают в 2026 и что делать нам

Ваше уточнение меняет вывод: **проверки версии недостаточно**, и это подтверждается всеми четырьмя
реализациями. Ниже — сравнение и конкретный план.

### 7.1 Четыре реализации, одна идея

| Проект | Формула id | Где stamp'ится | Где передаётся | Что сравнивают | «Пусто/неизвестно» |
|---|---|---|---|---|---|
| cline | `sdk-v1-sha256(<runtime sources>+lock+manifests)` (`runtime-build-id.ts:41-66`) | сборка бандла, `__CLINE_CORE_RUNTIME_BUILD_ID__` | hub discovery record | `buildId` == + `compareHubBuilds` (`index.ts:244`) | `missing_build` → отдельная причина |
| codex | `sha256("git:<40hex>:<target>")` (`build-info/lib.rs:131`) | `option_env!("STABLE_GIT_COMMIT")` на финальном крейте (`lib.rs:21`) | `EnvironmentInfo.provider_id` в `initialize` | `provider_id` + `server_version_notice_kind` (`update_versions.rs:30`) | commit `"dev"` / target отсутствует → `None` |
| grok-build | `"<version> (<12hex>)"` (`pager-bin/build.rs:30`) | `build.rs` c `rerun-if-changed=HEAD` | `liveLeaderInfo.leader_binary_version` | `leader_is_older_than` (`leader/mod.rs:105`) + mismatch-нотификация | unparseable → **не понижаем** (`false`) |
| omp | `{version, commit, branch, diffHash, label}` (`runtime-identity.ts:14`) | резолвится в рантайме из git-чекаута | session entries (не handshake!) | `sameRuntimeIdentity` (`:96`), `diff: "unknown"` = wildcard | `unknown` матчит любой diff того же commit |

Три вывода, которые переносятся к нам:

1. **Content hash лучше commit hash** для нашей ситуации. cline хэширует **исполняемый код** и явно
   исключает тесты (`runtime-build-id.ts:26-38`), поэтому их id меняется от правки рантайма без
   бампа версии — ровно наш случай «пересобрали ту же версию с патчем».
   codex хэширует commit+target и **честно** пишет, что это не байты (`build-info/lib.rs:128-130`).
   Для нас: commit-хэш дешёвый, но «грязное дерево» (`+dirty`) его обесценивает; content-hash по
   `crates/**` дороже, но честнее. Компромисс — §7.3.
2. **Stamp должен жить в финальном бинаре, не в библиотеке** (grok `version/lib.rs:20-22`,
   codex `build-info/lib.rs:16-20`): иначе git-изменение инвалидирует весь граф пересборки.
   У нас `pa-cli` — composition root, идеальное место для `build.rs`.
3. **Total order обязателен, если решение — «убить и перезапустить»**. Без него два процесса
   с разными сборками убивают друг друга (cline `index.ts:238-243`).

### 7.2 Что у нас уже есть (использовать, не изобретать)

- **`ALLSPARK_FORK_BUILD`** — это уже «stamp через `option_env!`», ровно приём codex:
  `crates/pa-cli/src/config.rs:61` `pub fn fork_build() -> &'static str { option_env!("ALLSPARK_FORK_BUILD").unwrap_or("dev") }`;
  `crates/pa-cli/src/lib.rs:240` печатает его в `--version`;
  `crates/pa-core/src/update/installer.rs:284,763` прокидывает в установщик.
  Проверено локально: `option_env!` действительно меняет вывод при пересборке
  (`PROBE_STAMP=A` → `Some("A")`, `PROBE_STAMP=B` → `Some("B")`, cargo пересобирает).
  **Расширить этот приём, не вводить второй.** (Плюс `docs/fork-identity.md:116`, `FORK.md:31`
  уже описывают его как контракт.)
- **`doctor` уже печатает `status: "stale"` + `executablePath`** — `crates/pa-cli/src/daemon_discovery/mod.rs:416`
  `classify_reachable` (protocol+schema+`config::version()`), поля `DaemonInfo.build_id`
  (`mod.rs:100`) и `probe.build_id = runtime.buildId` (`mod.rs:363`).
- **`runtime.buildId` уже на проводе**: `crates/pa-types/src/daemon/outbound.rs:146` `DaemonRuntimeIdentity { build_id, executable_path, ... }`,
  hello-вариант `outbound.rs:252`, заполняется в `crates/pa-daemon/src/supervisor/clients.rs:126`:
  `build_id: concat!("pa-daemon-rs-", env!("CARGO_PKG_VERSION"))`.
  **Это и есть корень проблемы: поле есть, но оно равно версии.**
- **`ensure_daemon_running` уже умеет `shutdown_stale_daemon`** —
  `crates/pa-cli/src/interactive_mode/daemon.rs:58-116`, включая отказ при busy-сессии
  («the running daemon has active work», `daemon.rs:~140`).
- **PTY-инфраструктура**: `crates/pa-perf/src/bench/pty.rs:28` `PtyChild`, `:214` `wait_screen_contains`,
  vt100-декодер; `openpty` используется в 17 файлах дерева (15 из них — `crates/pa-cli/tests/*`, плюс `pa-daemon/tests/multi_client_e2e.rs` и сам `pa-perf`). Отдельный тест «подменённый
  build-id» писать не с нуля.
- **Релизный манифест уже содержит хэш артефакта**: `scripts/release/assemble_artifacts.py:457-458`
  `"sha256": sha256_file(decoder), "executableSha256": sha256_file(binary)`;
  `:425` `gnu_build_id()` читает GNU build-id через `readelf -n` (используется только для decoder).
  Готовый фундамент для §7.6.

### 7.3 Предлагаемая форма build id для нас

Нужно три свойства и одно ограничение:

- **P1** меняется при правке любого крейта, влияющего на поведение демона/CLI;
- **P2** не меняется от правки тестов/доков/`.research`;
- **P3** детерминирован (две сборки одного дерева на одной машине → одинаковый id) —
  иначе «stale» будет срабатывать ложно;
- **ограничение**: `CARGO_BUILD_JOBS=2`, shared `target/`, никакого `cargo clean` (AGENTS.md).

Вариант A (рекомендуемый, дешёвый, честный) — два источника, одна функция:

```
build.rs в crates/pa-cli  (единственный build.rs в workspace; composition root)
  let stamp = match std::env::var("PA_BUILD_ID_STAMP") {   // CI/релиз: конвейер знает правду
      Ok(s) if !s.trim().is_empty() => s,                  // напр. "0.10.2-beta.3+g9f2a1c4"
      _ => derive_from_git(),                              // локальная сборка
  };
  cargo:rustc-env=PA_BUILD_ID={stamp}
  cargo:rerun-if-env-changed=PA_BUILD_ID_STAMP,ALLSPARK_FORK_BUILD,SOURCE_DATE_EPOCH
  cargo:rerun-if-changed=.git/HEAD, .git/logs/HEAD, <текущая ветка>   // приём grok build.rs:13-22
  cargo:rerun-if-changed=crates                                        // только ради dirty-diff

derive_from_git():
  commit  = git rev-parse HEAD                       (нет .git → "nogit")
  diff    = sha256(git diff HEAD --binary)           (пусто → "clean", >64 MiB → "unknown")
  dirty   = if diff != "clean" { "+d" + diff[..8] } else { "" }
  stamp   = "{version}+g{commit[..8]}{dirty}"        (omp-подобно: runtime-identity.ts:31,33,46-57)
```

Почему так:
- **Релиз не зависит от git-состояния машины сборки** — CI передаёт `PA_BUILD_ID_STAMP`; это же
  значение печатает `--version` и кладёт в манифест, т.е. три утверждения §7.6 берут один вход.
- **Локальная сборка различает грязное дерево** (`+d<8hex>` от `git diff HEAD`), а не только флаг
  «dirty». Это важно: два разных патча одного коммита получат разные id. Формулу diff-хэша я взял
  у omp (`runtime-identity.ts:31,33,46-57`: `diff`, `UNKNOWN_DIFF` при >64 MiB, wildcard-семантика),
  но, в отличие от omp, у нас он **compile-time**, потому что рантайм-резолв читает текущее дерево,
  а не то, из которого собрали бинарь (omp резолвит в рантайме осознанно: у него источник — bun-исходники
  рядом, у нас — `target/dist` без исходников).
- `rerun-if-changed=crates` — цена: правка любого `.rs` перезапускает build.rs `pa-cli`. Это **не**
  пересборка workspace (build.rs перезапускается один раз, десятки мс), но выглядит как «лишняя работа»
  в логах; альтернатива — не хэшировать diff и жить с флагом `-dirty` (дешевле, грубее).

Плюсы A: не зависит от `target/`, 1-2 `git` вызова, различает «та же версия, другой коммит»
и «та же версия, другой патч». Минус: различие «тот же коммит, разные feature-флаги» не ловится —
для этого вариант B.

Вариант B (cline-подобный, полный): `sha256` по всем `crates/**/*.rs` + `Cargo.toml` + `Cargo.lock`,
исключая `tests/`, `benches/`, `examples/`. Ловит feature-флаги и локальные патчи без git.
Цена: чтение ~1.5k файлов на каждой сборке `pa-cli`; **UNVERIFIED** по времени, но по аналогии с
`collectRuntimeSourceFiles` — десятки мс, приемлемо. Минус: `rerun-if-changed` придётся ставить на
каталог (`crates`), что заставляет build.rs перезапускаться чаще, чем нужно (но перезапуск build.rs —
это не пересборка крейта, только повторный запуск скрипта).

Рекомендация: **A сейчас** (2 часа работы), **B — если A окажется недостаточно** (например, когда
начнём собирать с разными feature-флагами в CI). Держать формулу в одном месте
(`pa_cli::config::build_id()`), чтобы `--version`, hello и манифест читали одно и то же.

Формат строки: `allspark <version> (fork build fork.NNN; build <id>; upstream prime-agent <version>)`.
**Осторожно**: `install-rust.sh:2805-2840` парсит второй токен и срезает `(fork build ...)` по
`install-rust.sh:2817,2820`; `verify_release.py:216-224` делает то же (`head.rsplit(" ",1)[-1]`).
Добавление поля **сломает** оба парсера, если ставить его до версии. Ставить **после** `fork build`
и добавить по тесту в `crates/pa-cli/tests/installer_stop_gate.rs` / `packaged_layout_e2e.rs:395`
(они уже парсят эту строку).

### 7.4 Что сравнивать в рантайме

Не «равны ли id» — это сломает `target/debug` рядом с `target/dist` в одном дереве (**и это норма**:
разработчик так и работает). Сравнивать надо **направленно**, как grok/cline:

```
enum DaemonIdentity {
    Same,                    // protocol+schema+build id совпали
    OlderThanClient,         // детерминированно старее → можно заменить (если не busy)
    NewerThanClient,         // новее → НЕ трогать, сказать пользователю (у него новее)
    Incomparable,            // id отсутствует/непарсится → вести себя как сегодня
}
```

Правила:
- `Same` → работаем (сегодняшнее поведение).
- `OlderThanClient` → `shutdown_stale_daemon` (он уже есть, `daemon.rs:112`), с отказом при busy.
- `NewerThanClient` → **не** убивать; напечатать баннер в стиле grok/cline
  (`"Daemon runs build {x}, this client runs {y} — restart the window to pick up the update"`,
  ср. `daemon_reconnect.rs:32` — у нас уже есть баннер «version-honest reconnect», его и расширить).
- `Incomparable` (старый демон без build id) → сегодняшнее поведение + **debug-лог**, чтобы не
  ломать совместимость с уже запущенными демонами после апгрейда.

Ключевая проверка (fail-safe): «неизвестно» никогда не должно приводить к убийству процесса —
это прямо записано в grok `leader_is_older_than` (`leader/mod.rs:105-113`, unparseable → `false`)
и в его тесте (`!leader_is_older_than("unknown","0.2.0")`).

### 7.5 Три уровня тестов (по вашей матрице tier'ов)

| Tier | Тест | Что доказывает | Красный на текущем коде? |
|---|---|---|---|
| Unit | `build_id_changes_when_a_runtime_source_changes` / `ignores_test_only_changes` / `is_deterministic` | формула P1-P3 | да (функции нет) |
| Unit | `identity_order_is_antisymmetric_and_transitive` (proptest) | total order корректен | да |
| Unit | `unknown_build_id_never_evicts` | fail-safe | да |
| Unit | `older_daemon_is_evicted_newer_is_kept` (как `decide_relaunch_is_idempotent_and_directional`) | направленность | да |
| Integration | два фейковых hello (`daemon_hello` с разными `runtime.buildId`) через **существующий** мок-супервизор (образец: `crates/pa-tui/tests/model_picker_provider_headless.rs:60-110` — `MockSupervisor` на `UnixListener`) | `probe_daemon` и `classify_reachable` дают **одну** классификацию | да — сейчас они расходятся (§8.2) |
| Integration | `hello_without_build_id_falls_back_to_version` | совместимость | да |
| PTY e2e | бинарь A запускает демон, бинарь B (с подменённым `PA_BUILD_ID` через env на **дочернем** процессе — не на своём) подключается → видит баннер, **не** убивает демон; затем idle-демон заменяется | сквозной путь | да |

Важная деталь из codex (`exec-server/tests/initialize.rs:26-40`): тест **специально** выставляет
`STABLE_GIT_COMMIT`/`GITHUB_SHA` в env дочернего процесса и утверждает, что runtime env **не подменяет**
stamp. Нам нужен обратный, но симметричный тест: env **подменяет** stamp только для тестовых бинарей
и только если это документировано (`CLINE_HUB_BUILD_ID` — их аналог, `hub/discovery/index.ts:122`).
Решение: `PA_BUILD_ID` — **compile-time** (`option_env!`), а для тестов — отдельная env-переменная
`PA_BUILD_ID_OVERRIDE`, читаемая только в `cfg(debug_assertions)` или только тестовым бинарём.
Иначе появится путь «пользователь выставил env и обошёл проверку».

### 7.6 Опубликованный артефакт: связать `--version`, build id и sha256

Три независимых утверждения, все должны сходиться:

1. **`--version`** печатает build id (compile-time) → `install-rust.sh:2820` уже читает строку,
   `verify_release.py:205-230` уже сверяет версию с `package.json`.
2. **Манифест канала** (`latest.json`, `release-fork.yml:195-206`) содержит `sha256` архива + `version`.
   **Добавить** `"buildId": <id>` — тогда `install-rust.sh` может сверить «скачал архив, чей манифест
   обещает buildId X» с «`--version` говорит X». Сегодня: sha256 сверяется (`install-rust.sh:1596-1603`),
   версия сверяется (`:2820`), а связи «версия ↔ конкретная сборка» нет.
3. **`binaries.json`/`manifest.json`** (`assemble_artifacts.py:496-503` — запись `entry`, `:506-515` — merge) уже несёт `sha256` и
   `executableSha256`. Добавить `buildId` рядом с ними — это одно поле и один assert в `verify_release.py`.

Дополнительно (дешёво, `gnu_build_id` уже написан): для Linux публиковать **GNU build-id** в манифесте
(`assemble_artifacts.py:425` — функция есть, применяется только к decoder). Тогда проверка
опубликованного бинаря — `readelf -n` на скачанном файле vs манифест, без запуска.

Что **не** надо делать: пересчитывать sha256 бинаря в рантайме (дорого, 70 МБ) или хэшировать
`current_exe()` при каждом старте. cline хэширует **исходники на сборке**, codex — commit+target на
сборке; оба не читают свой бинарь в рантайме.

### 7.7 Миграция без поломки существующих демонов

Уже запущенные демоны (включая наш живой на `pid 3289830`, старый `0.10.1-beta.35`) **не имеют**
нового id. Порядок:

1. Новый клиент видит hello **без** поля → `Incomparable` → сегодняшнее поведение (доктор скажет `stale`
   по `appVersion`, как сейчас).
2. Новый клиент видит hello **с** `runtime.buildId`, который не парсится как наша схема →
   `Incomparable` + warn.
3. Только когда обе стороны новой сборки — `Same`/`Older`/`Newer`.

Это ровно policy grok («unparseable → leave alone») и codex («`+dev` не считается старее»).

---

## 8. Что это значит для нашего дерева (file:line, проверено)

### 8.1 Уже есть (не предлагать заново)

| Что | Где |
|---|---|
| `doctor` с `status: Current|Stale` и `executablePath` | `crates/pa-cli/src/daemon_discovery/mod.rs:416`, `:363`; CLI-вход `crates/pa-cli/src/public_command.rs:172,457` |
| `ensure_daemon_running` + `shutdown_stale_daemon` (отказ при busy) | `crates/pa-cli/src/interactive_mode/daemon.rs:58,74,112` |
| hello несёт `runtime.buildId`/`executablePath` | `crates/pa-types/src/daemon/outbound.rs:146,252`; заполнение `crates/pa-daemon/src/supervisor/clients.rs:126` |
| Version-honest reconnect banner | `crates/pa-tui/src/daemon_reconnect.rs:32,60` |
| PTY + vt100 harness | `crates/pa-perf/src/bench/pty.rs:28,214`; `openpty` в 15 файлах `crates/pa-cli/tests/` + `pa-daemon/tests/multi_client_e2e.rs` |
| Релизный гейт каталога с форк-провайдерами | `scripts/release/bundle_catalog.py:117,202-211` |
| `--version` парсится установщиком и verifier'ом | `install-rust.sh:2805-2840`; `scripts/release/verify_release.py:205-230` |
| Форк-модели держатся на `.chain(fork_provider_models())` | `crates/pa-core/src/models/registry.rs:600` |
| Тесты на форк-таблицу | `crates/pa-core/src/models/registry/tests.rs:618,632,656` (в рабочем дереве, uncommitted) |
| `--fixture`/`verify` гейт каталога | `scripts/release/bundle_catalog.py:808-850`, `scripts/release/test_catalog_assets.py:202` |

### 8.2 Два классификатора демона расходятся (подтверждаю, с номерами строк)

- `crates/pa-cli/src/daemon_discovery/mod.rs:416-425` — `classify_reachable`:
  `protocol_version == 7 && schema_id == DAEMON_SCHEMA_ID && version == Some(config::version())` → `Current`.
  Используется **только** `doctor`/`ps` (`daemon_discovery/stop.rs:28` `run_ps`, вызывается из
  `public_command.rs:450,468`).
- `crates/pa-cli/src/interactive_mode/daemon.rs:31-46` — `probe_daemon`:
  `protocol.version == 7 && schemaId == DAEMON_SCHEMA_ID` → `Current`. **`appVersion` не читается**,
  build id не читается. Это путь запуска TUI (`interactive_mode.rs:97`) и `-p`-пути
  (`print_runtime.rs:104`).
- Третий, частичный: `crates/pa-tui/src/daemon_client.rs:447` сверяет только `protocol.name`,
  `:456` берёт `version.min(DAEMON_PROTOCOL_VERSION)` — то есть **сам клиент никогда не отказывается**
  от соединения со старым демоном, если protocol.name совпал.
- Итог для одного и того же живого демона: `doctor` → `stale`, `probe_daemon` → `current`,
  `DaemonClient::connect` → соединение установлено. Это подтверждается вашими живыми замерами.

**Правка**: один `pub(crate) fn classify(hello: &Value, client: &ClientIdentity) -> DaemonIdentity`
в `pa-types` (или `pa_cli::daemon_identity`), вызываемый из всех трёх мест; тест «оба пути дают одну
классификацию» на двух фейковых hello (мок-супервизор уже есть в `crates/pa-tui/tests/*`).

### 8.3 Гейт `FORK_PROVIDERS` сейчас красный на `--fixture` — воспроизведено

Я запустил ровно то, что делает `release-fork.yml:127-129` (в `.tmp`, без сети):

```
$ python3 scripts/release/bundle_catalog.py generate --fixture --out .tmp/report-catalog-assets
{"models": {"models": 46, "transportTuples": 43}, "mcpServices": {"services": 73}, ...}   # exit 0
$ python3 scripts/release/bundle_catalog.py verify --out .tmp/report-catalog-assets
error: Bundled model catalog is missing the fork's providers ['codebuddy','genspark','muse','ninerouter','subquadratic']:
       a release asset must carry them (chain.rs step 2 replaces the compiled catalog with this file)   # exit 1
```

И существующий тест тоже красный:

```
$ python3 -m pytest scripts/release/test_catalog_assets.py -k fixture_passes_the_full_gates -x -q
FAILED ... AssertionError: 1 != 0 : error: Bundled model catalog is missing the fork's providers [...]
```

Почему: `generate --fixture` ставит `allow_small = options.allow_small_fixture or options.fixture`
(`bundle_catalog.py:842`), то есть **сам себя освобождает** от гейта, а `verify` — нет.
`release-fork.yml:127` вызывает `generate --fixture` (проходит), `:129` — `verify` (падает).
Это **не** наш инцидент, это второй, свежий: гейт добавлен в рабочем дереве (uncommitted,
`git diff scripts/release/bundle_catalog.py`) и не согласован с `--fixture`.

Форма дефекта — та же, что §5.3: **тест проверял не тот режим**. Правка: либо `FORK_PROVIDERS`
должен присутствовать и в fixture (`fixture_catalog_bodies()` — добавить 6 строк-заглушек с реальными
`(provider, api, baseUrl)` из `models_fork.rs`), либо `--fixture` не должен освобождать от
fork-провайдеров (`allow_small_fixture` влияет только на **count**-гейты, не на provider-гейт).
Второе честнее: количество — это про «не обрезанный фикстур», а провайдеры — про «ассет того продукта».

Дополнительный факт к вашему пункту 3: я сверил fixture-транспорты с compiled-таблицей —
**все 43 fixture-кортежа присутствуют в `models_generated.rs`** (0 отсутствующих), т.е.
`pin_catalog_models` (`crates/pa-models/src/pinning.rs:68`) **не отбросит** fixture-строки;
отбрасывание случится на другой категории моделей. И 46 fixture-моделей **не пересекаются** ни с одной
compiled-моделью по `(provider,id)` (0 пересечений) — то есть на fixture-каталоге пользователь видит
46 «Fixture …» моделей и **ни одной настоящей** (`glm-4.7` для `zai` заменён на `fixture-zai-42`).
Это подтверждает вашу формулировку «в релиз едет синтетический каталог» — и добавляет, что он
**не будет отброшен pinning'ом целиком**, а именно **заменит** настоящие.

### 8.4 Пробелы по обеим темам, с приоритетом

**P0 (это инцидент):**

1. **Нет build id.** `clients.rs:126` — `concat!("pa-daemon-rs-", env!("CARGO_PKG_VERSION"))`.
   Ни одного `build.rs` в `crates/`; `rg 'GIT_HASH|git_sha|commit_hash|SOURCE_DATE'` → 0.
   `DAEMON_SCHEMA_ID` (`pa-types/src/daemon/mod.rs:17`) — ручной литерал.
   → §7.3 вариант A.
2. **Нет non-empty инварианта на пути «каталог → реестр».** `crates/pa-core/src/models/registry.rs:588-620`
   `load_built_in_models` делает `.resolve().chain(fork).map(...).collect()` — результат может быть пустым
   (если bundled пуст и fork-таблица пуста) и никто об этом не скажет. Инвариант нужен **внутри**
   функции (как `assertNonEmptyModels`), а не в тесте.
3. **Два классификатора** (§8.2). Один, с build id в составе.

**P1 (ловит класс ошибки):**

4. **`bundled_asset_beats_compiled_fallback`** (`chain.rs:346`) проверяет, что bundled победил,
   но **не** проверяет, что после победы fork-модели на месте. Тест `bundled_asset_keeps_fork_models`
   (bundled с 1 моделью → registry всё равно листает все fork-модели) — дешёвый и красный
   на текущем коде, если делать через `ModelCatalog::with_bundled_dir` + `load_built_in_models`.
5. **`test_fixture_passes_the_full_gates` + `verify`** — согласовать (§8.3).
6. **`--version` ↔ манифест**: добавить `buildId` в `latest.json` (`release-fork.yml:195-206`) и в
   `binaries.json`/`manifest.json` (`assemble_artifacts.py:496-503`), assert в `verify_release.py` и в `install-rust.sh`
   после `--version`-пробы (`:2820`).

**P2 (гигиена/наблюдаемость):**

7. **Список непокрытого** (`docs/TESTING_MATRIX.md`-аналог) — у нас 17 QA-тасков (13 RT + 4 ST) и 235 `crates/*/tests/*.rs`,
   но нет документа «что не покрыто и почему». OpenHands это делает.
8. **Таксономия провалов** для `scripts/qa/run_suite.py` (cline `cline-failures.yaml`) —
   сейчас есть `failure reasons` в манифесте (`qa-tasks/README.md`), но без классов.
9. **PTY-сценарий на пикер** по образцу `dashboard_model_list_click.yaml` — у нас есть
   `crates/pa-tui/tests/model_picker_provider_headless.rs` (headless) и
   `crates/pa-cli/tests/interactive_fork_e2e.rs` (PTY), но нет сценария «открыть `/model` → увидеть
   fork-модель → выбрать».

### 8.5 Что из чужого НЕ переносить

- **codex «нет каталога → не стартуем»** — ломает наш no-cold-start (`chain.rs:1-6`).
- **grok «сериализовать refresh под мьютексом и ждать»** — у нас refresh fire-and-forget по
  критическому пути (AGENTS.md «Performance and the critical path»), нельзя.
- **omp `sameRuntimeIdentity` с `unknown`-wildcard для handshake** — у omp это про session entries,
  не про процессы; wildcard в решении «убить ли демон» опасен (неизвестный id не должен разрешать kill).
- **Полный content-hash (вариант B) сразу** — измерять сначала (AGENTS.md «Measure before optimising»).

---

### 8.6 Три конкретных изменения с критерием приёмки

| # | Изменение | Файлы | Критерий приёмки (красный до, зелёный после) |
|---|---|---|---|
| C1 | `build.rs` в `crates/pa-cli` → `option_env!`-stamp `PA_BUILD_ID`; `pa_cli::config::build_id()`; строка `--version`; поле `runtime.buildId` = build id; `latest.json`/`binaries.json` несут то же | `crates/pa-cli/build.rs` (новый), `crates/pa-cli/src/config.rs:61`, `crates/pa-cli/src/lib.rs:240`, `crates/pa-daemon/src/supervisor/clients.rs:126`, `.github/workflows/release-fork.yml:195-206`, `scripts/release/assemble_artifacts.py:496-503`, `scripts/release/verify_release.py:205-230` | `cargo test -p pa-cli --lib config` красный до (функции нет); `python3 scripts/release/verify_release.py` падает на артефакте без buildId |
| C2 | Один `classify_daemon_identity(hello, self)` в `pa-types`/`pa-cli`; `classify_reachable` (`daemon_discovery/mod.rs:416`), `probe_daemon` (`interactive_mode/daemon.rs:31`) и `DaemonClient::connect` (`pa-tui/src/daemon_client.rs:447`) вызывают его; направленное решение (`Same/Older/Newer/Incomparable`) | те же + `crates/pa-tui/src/daemon_client.rs:447-470` | integration-тест на двух фейковых hello: оба пути дают одну классификацию; при `Newer` — ни одного `Shutdown` |
| C3 | Non-empty инвариант на пути каталог→реестр + громкий сигнал, когда bundled отброшен/пуст | `crates/pa-core/src/models/registry.rs:588` (assert + warn), `crates/pa-models/src/chain.rs:165-190` (warn при `load_bundled_models` → `None`) | `bundled_asset_keeps_fork_models` и `empty_bundled_is_reported` красные до |

Порядок: C3 → C2 → C1 (C2 без C1 работает на версии, но не ловит пересборку; C1 без C2 бесполезен
для TUI-пути). Каждое — отдельный PR (AGENTS.md «One logical change per PR»), и каждое затрагивает
`pa-types`/`pa-cli` composition root — то есть C1 и C2 меняют `pub` surface только в `pa-types`
(новый тип), что по AGENTS.md требует строки в PR про ownership compliance.

## 9. Короткий ответ на «что мы не делаем и стоит ли перенять»

| Практика | Мы сейчас | Перенять? | Почему |
|---|---|---|---|
| Build id ≠ version | нет поля вообще | **да, P0** | наш живой случай: два бинаря, один `schemaId`, одна `appVersion` при пересборке |
| Total order сборок | нет | **да** | иначе авто-замена stale-демона станет «кто кого перезапустит» |
| `missing_build`/`build_mismatch` как причины | нет | **да** | делает «старый демон» наблюдаемым |
| Один классификатор на всех путях | два, расходятся | **да** | сегодня `doctor` говорит `stale`, TUI идёт в него молча |
| Non-empty assert в конструкторе | нет (пустота легальна в TUI-тесте) | **да, для каталога** | `an_empty_catalog_opens_the_empty_panel` документирует пустоту как норму — но не говорит, когда она баг |
| Пусто → своё сообщение | нет | **да** | grok `models_endpoint_empty_message` — 10 строк |
| Assert non-empty перед циклом в тесте | нет | **да** | наш `test_fixture_passes_the_full_gates` — вакуумный пример |
| Мок с request-log | faux без лога | **да (P1)** | `fork-testing-research.md` P0-1, здесь не повторяю |
| Декларативные PTY-сценарии | 4 PTY e2e, headless-тесты | **да, точечно** | пикер + `/model` + баннер версии |
| Список непокрытого | нет | **да** | дешёво, полезно |
| Таксономия провалов | нет | можно | после того как гейт стабилен |
| Replay прод-траекторий | нет (только TS golden) | можно | отдельная тема |
| Внешний PTY-инструмент | свой harness | **нет** | свой уже работает; зависимость не окупится |

---

## 10. UNVERIFIED / границы

- **UNVERIFIED**: детали `cline` `local-provider-service.test.ts` (назван DeepWiki; прочитал только
  `configured-provider-registry.ts`); что у grok-build есть feature-флаги, влияющие на поведение демона;
  точное время content-hash варианта B на нашем дереве.
- **UNVERIFIED**: что `keep_text_selection`-подобные config-ключи в `xai-grok-pager` не влияют на
  pipeline — не проверял, это вне темы.
- **Не запускал**: `cargo test`, `cargo build`, `install-rust.sh`, `bundle_catalog.py generate --network`,
  `allspark`/`prime-agent` в любой форме. Всё, что я запускал: `python3 scripts/release/bundle_catalog.py
  generate --fixture/verify` в `.tmp`, `pytest -k fixture_passes_the_full_gates`, и отдельная cargo-проба
  `option_env!` в `.tmp/buildid-probe` (удалена).
- **Не трогал**: живой демон `pid 3289830`, `~/.local/libexec*`, `~/.allspark-home`, сокеты.
- Числа «blobs/test-ish» — из GitHub recursive tree API на закреплённых коммитах; «inline `#[test]` = 7718»
  — `rg -c` по нашему дереву (включая `pa-perf`), может завышать на закомментированные.

---

## 11. Приложение: ссылки

Репозитории (URL с закреплённым коммитом):
- grok-build: https://github.com/xai-org/grok-build/tree/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8
- codex: https://github.com/openai/codex/tree/970111f6cb62cc02fe36a82ea7d71b37a2889c97
- oh-my-pi: https://github.com/can1357/oh-my-pi/tree/0b5c27ab24719c86a5986b95ee55f4e75bc1aaa0
- cline: https://github.com/cline/cline/tree/40c5d99031fe1128189c5b52e40638d2b7787754
- OpenHands: https://github.com/All-Hands-AI/OpenHands/tree/fee10e3d2909ebbf9950578575b1ee011492171e
- SWE-agent: https://github.com/SWE-agent/SWE-agent/tree/3ea751c087f32b16e039a2233dd6eefecef325d5
- aider: https://github.com/Aider-AI/aider/tree/5dc9490bb35f9729ef2c95d00a19ccd30c26339c
- tui-test: https://github.com/microsoft/tui-test/tree/f2760592ed00e90436f8c3bae124dab712eaecf8

Ключевые файлы (прямые ссылки):
- cline build id: https://github.com/cline/cline/blob/40c5d99031fe1128189c5b52e40638d2b7787754/sdk/packages/core/scripts/runtime-build-id.ts
- cline hub compat/order: https://github.com/cline/cline/blob/40c5d99031fe1128189c5b52e40638d2b7787754/sdk/packages/core/src/hub/discovery/index.ts#L122-L320
- cline non-empty registry: https://github.com/cline/cline/blob/40c5d99031fe1128189c5b52e40638d2b7787754/sdk/packages/core/src/services/llms/configured-provider-registry.ts#L47-L100
- codex build-info: https://github.com/openai/codex/blob/970111f6cb62cc02fe36a82ea7d71b37a2889c97/codex-rs/build-info/src/lib.rs#L16-L145
- codex catalog empty: https://github.com/openai/codex/blob/970111f6cb62cc02fe36a82ea7d71b37a2889c97/codex-rs/core/src/config/mod.rs#L2223-L2245
- codex server version notice: https://github.com/openai/codex/blob/970111f6cb62cc02fe36a82ea7d71b37a2889c97/codex-rs/tui/src/update_versions.rs#L28-L50
- codex empty-catalog test: https://github.com/openai/codex/blob/970111f6cb62cc02fe36a82ea7d71b37a2889c97/codex-rs/core/tests/suite/remote_models.rs#L1490
- grok build stamp: https://github.com/xai-org/grok-build/blob/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8/crates/codegen/xai-grok-pager-bin/build.rs
- grok leader ordering: https://github.com/xai-org/grok-build/blob/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8/crates/codegen/xai-grok-shell/src/leader/mod.rs#L95-L120
- grok default-models assert: https://github.com/xai-org/grok-build/blob/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8/crates/codegen/xai-grok-models/src/lib.rs#L26-L45
- grok picker scenario: https://github.com/xai-org/grok-build/blob/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8/crates/codegen/xai-grok-pager-pty-harness/tests/scenarios/dashboard_model_list_click.yaml
- omp runtime identity: https://github.com/can1357/oh-my-pi/blob/0b5c27ab24719c86a5986b95ee55f4e75bc1aaa0/packages/coding-agent/src/session/runtime-identity.ts
- omp catalog emptiness: https://github.com/can1357/oh-my-pi/blob/0b5c27ab24719c86a5986b95ee55f4e75bc1aaa0/packages/catalog/src/model-manager.ts#L370-L440
- OpenHands testing matrix: https://github.com/All-Hands-AI/OpenHands/blob/fee10e3d2909ebbf9950578575b1ee011492171e/docs/TESTING_MATRIX.md
- OpenHands catalog non-empty: https://github.com/All-Hands-AI/OpenHands/blob/fee10e3d2909ebbf9950578575b1ee011492171e/__tests__/constants/extensions-catalogs.test.ts#L9-L35
- OpenHands e2e guide: https://github.com/All-Hands-AI/OpenHands/blob/fee10e3d2909ebbf9950578575b1ee011492171e/.agents/skills/e2e-testing/references/guide.md
- cline smoke: https://github.com/cline/cline/blob/40c5d99031fe1128189c5b52e40638d2b7787754/evals/smoke-tests/README.md
- cline failure taxonomy: https://github.com/cline/cline/blob/40c5d99031fe1128189c5b52e40638d2b7787754/evals/analysis/patterns/cline-failures.yaml
- SWE-agent replay: https://github.com/SWE-agent/SWE-agent/blob/3ea751c087f32b16e039a2233dd6eefecef325d5/tests/test_run_replay.py
- aider model sanity: https://github.com/Aider-AI/aider/blob/5dc9490bb35f9729ef2c95d00a19ccd30c26339c/aider/models.py (sanity_check_model)
- tui-test: https://github.com/microsoft/tui-test/blob/f2760592ed00e90436f8c3bae124dab712eaecf8/README.md

Внешние источники (2025-2026):
- JSON Schema, `items` не запрещает пустой массив: https://json-schema.org/understanding-json-schema/reference/array
- dbt `at_least_one`: https://www.elementary-data.com/dbt-tests/at-least-one
- Great Expectations `expect_table_row_count_to_be_between`: https://greatexpectations.io/expectations/expect_table_row_count_to_be_between/
- Prometheus `absent()` / dead man's switch: https://www.skillveris.com/interview-questions/prometheus/alerting-on-missing-data-deadmansswitch
- Fail-open vs fail-closed: https://authzed.com/blog/fail-open
- Kubernetes version skew policy (почему точное равенство — не всегда цель): https://kubernetes.io/releases/version-skew-policy/
- SOURCE_DATE_EPOCH: https://reproducible-builds.org/docs/source-date-epoch/
- vergen (build-time env emission): https://docs.rs/vergen/latest/vergen/
- GitHub artifact attestations (для привязки артефакт↔сборка): https://docs.github.com/actions/security-for-github-actions/using-artifact-attestations/using-artifact-attestations-to-establish-provenance-for-builds
- Smoke testing как release gate: https://www.qawolf.com/blog/what-is-smoke-testing
