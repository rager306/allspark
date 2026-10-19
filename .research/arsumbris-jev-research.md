# arsumbris MCP ⇄ JEV / muse-code: research report

**Дата:** 2026-10-16 · **Режим:** read-only (кроме записи этого отчёта) · **Автор:** research subagent
**Артефакты:** `~/.arsumbris/*`, `/root/arsumbris/*`, `/root/.local/bin/muse-bin-1.4.9-R6297.1`, `/tmp/omp-src`, EXA search.

**TL;DR.** `arsumbris` — это **локальный типизированный knowledge-graph + agent-layer** (Volvelle Labs / «Ars Umbris», GitHub org `arsumbris`, релиз `0.0.1-alpha`), никак **не** связанный кодом с TypeSafe/JEV. `JEV` — это **System One модель TypeSafe** (`jev-latest`, endpoint `POST /v1/systemone`), которая в **другом** проекте — `omp` (oh-my-pi coding-agent, `/tmp/omp-src`) — занимает роль **`judge`-модели** (модельный role `modelRoles.judge`), возвращающей типизированные `noul`/`choice`/`score` решения. Связь arsumbris и JEV — **концептуальная, а не кодовая**: оба строят вокруг одного тезиса «агент принимает типизированные решения», и оба интегрируются как **MCP-серверы** в один и тот же харнесс. Проверка `strings` по muse-бинарю дала **0 совпадений** `arsumbris` и **0** `JEV`; в `/root/arsumbris` нет ни одной ссылки на TypeSafe/JEV (только ложные срабатывания в lock-файлах node_modules). Реальная рабочая связка в этой машине: **`arsumbris` MCP = bridge.py → au-mcp daemon на workspace `/root/vault`** (ZeroClaw vault), 49 инструментов; **JEV = judge-роль в omp**, отдельная экосистема.

---

## 1. arsumbris: архитектура (код, bridge, инструменты)

### 1.1 Что это

«A malleable, agent-native IDE for typed knowledge» (README). Три слоя, каждый — **отдельный git-репозиторий-сиблинг** под общим parent `/root/arsumbris/`:

| Слой | Репо | Роль |
| --- | --- | --- |
| **au-engine** | `au-engine` (+`au-engine-sdk`, Rust + TS) | Графовый движок над кросс-репо субстратом markdown/yaml. Свой typing language. Демон `au` (Unix socket). |
| **au-host** | `au-host` (Electron) | UI: «проекции» (views) над графом. |
| **au-mcp** | `au-mcp`, `au-mcp-sdk`, `au-mcp-core`, `au-mcp-adapter-cc`, `au-mcp-adapter-codex` | Agent-layer: ядро-демон + контракт + плагины + адаптеры харнессов. |

**Ключевой архитектурный принцип (decision 2608261517):** `au-mcp` — **kernel daemon, mechanism-only, ZERO tools**. Всё, что может агент, — это **плагин** над `au-mcp-sdk`. Даже governance-флоры (`read-guard`, `tool-precondition`, `native-tool redirect`) — это плагины, не привилегированный код.

### 1.2 Пайплайн

```
mediate -> act -> observe
```
- `mediate` — hooks-shape **mediator** решают allow/deny/inject/ask, по tier `gate -> floor -> policy`.
- `act` — тул исполняется через governed write seam (`broker.mutate`, un-forgeable stamp).
- `observe` — каждое событие пишется в per-session ledger.

Hooks-shapes: `observer | mediator | stamper | session-start`. Tools — это `mcp.tool`-сабтипы, чьи **поля = input-схема вызова**; runtime-мета (entry, access, description) едет в типизированном `meta:` (`plugin-runtime-meta`, `tool-access-meta`, `tool-presentation-meta`).

### 1.3 Установленный инстанс на этой машине

**Сокет и воркспейс.** `settings.json → mcpServers.arsumbris` запускает stdio-обёртку:
```
/bin/bash -c "AU_MCP_SOCKET=/root/.arsumbris/au-mcp/run/adcfe8680a485529.sock exec python3 /root/arsumbris/au-mcp-bridge/bridge.py"
```
Хэш `adcfe8680a485529` я проверил: это **FNV-1a-64 от `/root/vault`** (совпадает с `socketFileName` в `au-engine-sdk/src/client.ts`, который считает `contentHash(FNV-1a-64)` от realpath entry). **Значит боевой воркспейс — `/root/vault`**, ZeroClaw engineering knowledge vault (типизированный arsumbris-граф: `knowledge/`, `type/*.type.yaml`, `ops/`, `self/`). Зависит от `mcp-bundle`.

**Два демона (systemd + cron-хилчек).** `~/.arsumbris/bin/au-health.sh` каждые 15 мин проверяет `au daemon status /root/vault` и `au-mcp status /root/vault`, при падении перезапускает `arsumbris-engine.service` / `arsumbris-mcp.service`. Лог `~/.arsumbris/logs/health-checks.log` — сплошные `engine=1 mcp=1 action=ok` (стабильно).

**Конфиги:** `~/.arsumbris/au-engine/config/repos.yaml` — реестр 21 located-repo (все `au-*` + `vault` + defaults); `~/.arsumbris/au-host/config/paths.yaml` — пути бинарей; `~/.arsumbris/au-mcp/gen/skill/...` — материализованные скиллы для CC-адаптера (`build-a-plugin`, `build-a-skill`, `build-a-session-inject`).

### 1.4 bridge.py — как MCP-мост работает

`/root/arsumbris/au-mcp-bridge/bridge.py` (v2, 10650 B; рядом `bridge.py.v1.bak`) — **самописный stdio↔Unix-socket мост** (не часть upstream-репо; git-истории нет). Он нужен, потому что au-mcp говорит по **своему wire-протоколу** (4-байтный big-endian length prefix + JSON на Unix-сокете), а MCP-клиент ждёт **JSON-RPC over stdio**.

Что делает bridge:
1. **MCP surface**: `initialize` (protocolVersion `2025-06-18`), `tools/list`, `tools/call`; прочие `notifications/*` глотает, неизвестные методы → JSON-RPC error, никогда не падает.
2. **Name round-trip**: демон регистрирует callables как `mcp.au_overview`; MCP-клиент запрещает точку в имени, поэтому bridge отдаёт **safe-имя** `mcp__au_overview` (точки→`__`) и на `tools/call` маппит обратно в manifest-id.
3. **Session pinning**: `tools/list` открывает ОДНУ сессию демона, `tools/call` её переиспользует (чтобы freshness/continuity трекинг демона был когерентен). v1 создавал новую сессию на каждый list.
4. **Reconnect**: при transport-ошибке (демон рестартнул) сокет переподключается один раз и повторяет вызов; иначе возвращает legible `isError` вместо broken pipe.
5. **tools/call**: `invoke` → `{content:[{type:"text",text:...}], isError}`; не-string результат сериализуется в JSON.

Комментарии в bridge прямо называют клиента: **«Hermes MCP client»**. То есть изначально мост писался под Hermes (не под prime-agent) — форк переиспользовал его as-is. Это точка интеграции «форк ↔ arsumbris»: в `settings.json` арсумбрис подключён как обычный stdio-MCP.

### 1.5 Инструменты (49 callables, live из демона)

Демон жив (проверено прямым JSON-RPC на сокет). 49 tools: **14 read-write, 32 read, 3 `access:none`** (заглушки `bash`/`glob`/`grep_files`).

**Read — типизированный граф (`au_*`, 32 шт.):** `au_overview` (первый дешёвый read), `au_instances`, `au_instances_of(ofType=…)`, `au_types`, `au_type`, `au_type_tree`, `au_subtypes`, `au_references(out|in)`, `au_neighborhood` (N-hop walk), `au_hubs` («start here»), `au_frontmatter`, `au_resolved`, `au_typed`, `au_validate` (dry-run валидация без записи), `au_follow` (резолв wikilink — authoring-check), `au_files`, `au_anchors`, `au_block_ids`, `au_diagnostics`, `au_candidates` (type-induction), `au_children`, `au_top_level_dirs`, `au_members`, `au_resolve_member`, `au_list_imports`, `au_semantic_tokens`, `au_type_system`, + 4 host-intent tools.

**Write (14, governed seam):** `write_file`, `edit_file`, `delete_file`, `read_file_pinned` (governed read), `rename` (атомарная saga с перезаписью всех inbound-ссылок), `rename_type`, `rename_block_id`, `assign_block_id`, `promote`/`inline` (extract/fold inline `^:` record), `add_dep`/`remove_dep`, `add_workspace_member`/`remove_workspace_member`/`set_workspace_member_role`.

**Governance-флоры (не tool, а hooks):** `read-guard` (нельзя перезаписать непрочитанный/устаревший файл), `tool-precondition`, `nativeToolRedirect` (tier gate). Session-start hooks: `repo-overview`, `instance-count-notice`, `requiredScope-check`.

**Agent-profile** — именованный bundle: `skills`, `inject`, `tools` (allowlist), `nativeToolAllowlist`, `hooks`, `hookConfig`, `adapter`. Инструменты видны агенту через profile (visibility daemon-side, не env-allowlist).

---

## 2. Связь с muse-code / JEV экосистемой

### 2.1 Прямая кодовая связь: ОТСУТСТВУЕТ (проверено)

| Проверка | Результат |
| --- | --- |
| `grep -riI 'arsumbris'` в muse-бинарe | **0** |
| `grep -riI 'JEV'` (case-sens) в muse-бинарe | **0** |
| `grep -riI 'jev'` в `/root/arsumbris` (excl node_modules) | **0** |
| `grep -riI 'muse'` в `/root/arsumbris` | **0** |
| `grep -riI 'typesafe'` в `/root/arsumbris` | **0** |
| `grep -riI 'typesafe|jev|judge'` в `/root/prime-agent/crates` | **0** |

Совпадения «jev» в `/root/arsumbris` были **только** в `pnpm-lock.yaml` (base64 integrity-хэши) — ложные срабатывания.

### 2.2 Что такое muse-code (из бинаря)

`muse-bin-1.4.9-R6297.1` (372 MB) — **Meta «Muse Code»**, реализация **MSP (Muse Session Protocol) v1** — «rendered from the tbh-protocol schema model». Rust-крейты в строках: `tbh_tui`, `tbh_agent`, `tbh_session_server`, `tbh_session_view`, `tbh_protocol`, `tbh_config` — paths `fbcode/musecode/build/src/crates/...`. Это **TUI coding-agent** (аналог Claude Code) с subagent-tree, workflow API v2, approval-judge, skills, MCP-серверами (`mcpServers` в settings, OAuth-flow), model catalog (roles `default/smol/slow/judge/...`). Провайдеры — Meta API (`api.meta.ai`), но поддерживает Anthropic/OpenAI/OpenRouter/Gemini.

**Прямой связи arsumbris↔muse в бинаре нет.** Общего у них — только то, что оба умеют подключать **MCP-серверы** и оба — coding-agent харнессы.

### 2.3 Что такое JEV (TypeSafe System One)

**JEV** — флагманская **System One** модель TypeSafe AI (`jev-1.14.0`, `jev-latest`; endpoint `POST https://api.typesafe.ai/v1/systemone`, Bearer). Это **не чат-LLM и не code-completion**: она не пишет текст, а **принимает state + карту типизированных вопросов** и возвращает **структурированные ответы**:
- `noul` — калиброванная вероятность 0–1 для утверждения (yes/no);
- `choice` — выбор из закрытого списка, с per-option вероятностями;
- `score` — оценка по рубрике (2–10 уровней).

Доступна также через OpenRouter (`~typesafe/jev-latest`). Родственные System One модели: OpenRouter/OpenAI **Decisions**, Workers AI **Clef**.

### 2.4 Где JEV реально используется: `omp` (oh-my-pi), `/tmp/omp-src`

Единственная найденная рабочая связка «JEV как judge в coding-agent» — это **omp** (не muse, не prime-agent). В `omp` JEV — **первоклассная judge-роль**:

- `packages/coding-agent/src/priority.json` → `"judge": ["typesafe/jev-latest", "openrouter/~typesafe/jev-latest", "@tiny", "@smol", "@default"]`.
- `packages/coding-agent/src/config/model-roles.ts` → роль `judge` (`acceptsJudge`: `kind==="judge"|"tiny"|"chat"`).
- `packages/coding-agent/src/judgment/index.ts` → `ChainJudge`, `judgeRoleChain`, `kindOf()` (`native` для `isJudgmentApi(model.api)` — TypeSafe/Decisions/SystemOne; `local`; `online`). Есть **answer-cache** (`judgment/cache.ts`, SQLite: states/oracle/usage).
- **TTSR (Time-Traveling Stream Rules)** — `export/ttsr.ts`: judged-rules (`question`) не матчатся mid-stream; сессия спрашивает judge-модель про каждый завершённый output и при `noul >= 0.7` инжектит правило-предупреждение. Есть token-бюджет под JEV: `countTokens(text, Encoding.Jev)`, `JUDGED_CONTENT_MAX_TOKENS = 32000` (JEV отвергает ветку >~33k токенов).
- `ttsr-settings.ts`: `ttsr.judge` enum `auto|on|off`, где `auto` = «judge only when the judge role resolves to a native TypeSafe jev model».
- Кастомные провайдеры: `... typesafe, openrouter-decisions, openai-decisions, workers-ai-systemone (the judge APIs)` в `~/.omp/agent/models.yml`.

**Вывод:** в coding-agent контексте JEV используется как **дешёвый быстрый судья/классификатор** на decision-boundary (проверить claim, отскринить контент перед попаданием в контекст, ранжировать кандидатов, гейтить «готово ли»), а не как основной агент. Публичная документация TypeSafe прямо это подтверждает: «Jev is not a drop-in replacement for the LLM behind Claude Code… use your coding agent as usual to write code that uses Jev to make decisions».

### 2.5 Экосистема JEV-MCP (внешняя, для контекста)

Существует много сторонних **JEV-MCP-серверов** (не связанных с arsumbris): `jkudish/jev-mcp` (12 tools: `jev_verify`, `jev_screen`, `jev_noul`, `jev_find`, `jev_rerank`, `jev_classify`, `jev_decide`, `jev_compare`, `jev_extract`, `jev_audit`, `jev_review`, `jev_gate`), `tumf/jev-cli`, `MarkChu-git/typesafe-mcp`, `tphakala/jev-mcp` (Go), JEV Hub, AutoJev и т.д. MLflow имеет встроенного `make_judge()` с `typesafe:/`-URI.

### 2.6 Связь arsumbris ↔ JEV: концептуальная (две стороны одной монеты)

| Ось | arsumbris | JEV (в omp) |
| --- | --- | --- |
| Тезис | «Knowledge should work like a codebase»; типизированный граф, агент принимает **типизированные решения** о структуре | «Fast, structured decisions for software»; модель выдаёт **типизированные решения** вместо текста |
| Что типизировано | Types/fields/refs в markdown+yaml; tools = типы | `noul`/`choice`/`score` — типы ответов |
| Роль в харнессе | **MCP-сервер**: даёт агенту граф знаний (context + governed writes) | **MCP-сервер ИЛИ judge-роль**: даёт агенту дешёвое решение |
| Общий слой | Оба подключаются к Claude Code / Codex / omp / muse как MCP | То же |

То есть arsumbris и JEV **комплементарны**: arsumbris = «типизированная память/знание + governed file-ops», JEV = «типизированное суждение на decision-boundary». Оба — «typed decisions for agents», отсюда и путаница в задании. **Прямой интеграции arsumbris×JEV нигде нет** (ни в репо arsumbris, ни в omp, ни в muse).

---

## 3. MCP инструменты: какие есть, как использовать

### 3.1 Как подключён в форке (текущее состояние)

В `~/.prime/agent/settings.json` **и** в `~/.allspark-home/.prime/agent/settings.json` арсумбрис зарегистрирован **идентично** как stdio-сервер:
```json
"arsumbris": {
  "type": "stdio",
  "command": "/bin/bash",
  "args": ["-c", "AU_MCP_SOCKET=/root/.arsumbris/au-mcp/run/adcfe8680a485529.sock exec python3 /root/arsumbris/au-mcp-bridge/bridge.py"]
}
```
Все 49 tools прилетают агенту как `mcp__arsumbris__mcp__au_overview` и т.п. (safe-name через `__`). Воркспейс — `/root/vault`.

### 3.2 Рабочие паттерны (что и когда звать)

- **Orientation:** всегда `au_overview` первым (дешёвая карта + diagnostics + graph-shape), затем drill-down.
- **Поиск знания:** `au_hubs` (start-here), `au_instances_of(ofType=...)`, `au_neighborhood(seed, hops)`, `au_references(which=in|out)`.
- **Авторинг/правка:** `au_typed`/`au_validate` (проверить до записи), `au_follow` (резолв wikilink перед записью), затем `write_file`/`edit_file` (governed seam — обязательно прочитать файл заранее из-за read-guard).
- **Структурные мутации:** `rename` (перепишет все inbound-ссылки атомарно), `promote`/`inline` (вынести/свернуть inline `^:`-запись).
- **Контроль качества:** `au_diagnostics` (ошибки/варнинги графа), `au_candidates` (какой тип мог бы заявить файл).

### 3.3 Ограничения (важно)

- `bash`, `glob`, `grep_files` — **заглушки** (`access:none`, «Not available yet (no shell)»). Shell-операций через арсумбрис нет.
- `access` влияет на то, какие tools видны в профиле; `read-write` требует прочитанного файла (read-guard).
- Мост однопоточный по сокету (lock) и пиннит одну сессию — при параллельных агентах возможна сериализация.

---

## 4. Рекомендации: как использовать arsumbris в форке allspark

1. **Оставить как есть — рабочий read/write-слой поверх vault.** Подключение корректно (stdio bridge, session-pinning, reconnect). Vault `/root/vault` уже типизирован и жив. Это даёт форку «вторую память» рядом с continual-harness: структурированный knowledge-graph с governed writes и diagnostics.

2. **Развести роли памяти.** Не дублировать vault в `pa-core` harness memory. Разделение:
   - **vault (arsumbris)** — долговременные типизированные артефакты (patterns/decisions/errors/insights, topic maps);
   - **continual harness (`pa-core/refinement`)** — оперативные prompt/memory/skill/subagent/factory-записи сессии.
   Мостик: скилл, который по завершении задачи предлагает вынести durable-урок в vault через `write_file` (с `au_validate` перед записью).

3. **Использовать governance-флоры как образец.** Паттерн «kernel mechanism-only + plugin floors + mediate→act→observe + un-forgeable stamp» из au-mcp — сильный референс для `pa-core` tool_profile/hooks. Стоит изучить `read-guard`/`tool-precondition` как готовую модель «deny write without read».

4. **Зафиксировать хрупкость контракта.** Мост самописный и вне git; хэш сокета зашит в settings. Рекомендации:
   - обернуть путь сокета в маленький resolver (FNV-1a-64 от realpath workspace), а не хардкодить `adcfe8680a485529`;
   - положить `bridge.py` под версионный контроль (сейчас только `bridge.py` + `.v1.bak` без истории) — иначе невоспроизводимо;
   - health-check уже есть (`au-health.sh` + systemd), но он привязан к `/root/vault`; при смене воркспейса обновлять.

5. **Проверить лицензионный/политический аспект.** `arsumbris` — early-alpha, **без sandboxing** («runs code on your machine», SAFETY.md), macOS-only (Linux «may work»). Форк на Linux уже использует его — зафиксировать это как поддерживаемый, но рискованный путь (arbitrary code execution через плагины/projections).

6. **Не ждать JEV-интеграции от arsumbris.** Если форку нужен «typed judge на decision-boundary», это **отдельная** интеграция: либо judge-роль (как в omp, через `pa-ai` custom provider `typesafe`/`systemone`), либо MCP-сервер `jev-mcp`. arsumbris сюда ничего не добавляет.

---

## 5. Gap analysis: чего не хватает

### 5.1 Разрыв «arsumbris × JEV» (главный gap задания)

- **Нет** ни одного JEV/TypeSafe-инструмента в arsumbris (проверено: 0 ссылок). arsumbris вообще не вызывает LLM/judge — он «engine + agent-layer», модель приходит из харнесса.
- **Нет** arsumbris-упоминаний в muse-бинаре и в omp. Связь — только концептуальная.
- **Вывод:** если цель — «arsumbris + JEV вместе», эту интеграцию нужно **построить**: (a) judge-роль `typesafe/jev-latest` в `pa-ai` custom providers, ИЛИ (b) MCP `jev-mcp` рядом с `arsumbris` MCP. Оба подключаются к allspark независимо.

### 5.2 Разрывы в самом arsumbris (по коду)

- **Нет shell-тула** (`bash` — заглушка). Всё file-ops/governance есть, shell — нет.
- **Нет model/judge-тула** в `au-mcp-core` (0 упоминаний model/judge в `src/`). Типизация есть, «суждения» — нет.
- **Нет versioned bridge** в upstream (мост — локальная самоделка).
- **macOS-only** официально; Linux-путь не гарантирован (а форк — Linux).
- **Нет sandbox** для плагинов/projections (SAFETY.md это признаёт).
- **`au_*` read-тулы богатые, но `au_candidates`/type-induction — эвристика**; не проверял качество на реальных данных.

### 5.3 Разрывы в форке относительно arsumbris

- Хардкод хэша сокета и воркспейса `/root/vault` в settings (нет абстракции workspace).
- Нет скилла/промпта, объясняющего агенту, **как** пользоваться `au_*`-графом (материализованные скиллы есть только для CC-адаптера, не для prime-agent).
- Нет моста между vault-knowledge и continual-harness memory (две несвязанные памяти).
- Профили (`agent-profile`) не используются: bridge открывает session-less advertise, tool-visibility = unrestricted.

### 5.4 Что проверить дальше (не входило в задание)

- Кто и как создал `/root/vault` (ZeroClaw — не Meta, не TypeSafe; сторонний проект).
- Реально ли `au-host` (Electron) поднят на этой машине (пути засеяны, но процесс не проверял).
- Живые `au_*`-запросы к непустому графу — качество diagnostics (в `au_overview` было 51 diagnostic: 35 error `required-field-absent`, 16 warning `navigational-target-not-found` — vault **частично невалиден**).

---

## Приложение: доказательства

- `~/.prime/agent/settings.json` и `~/.allspark-home/.prime/agent/settings.json` → `mcpServers.arsumbris`.
- `/root/arsumbris/au-mcp-bridge/bridge.py` (v2) + `.v1.bak`; комментарии про «Hermes MCP client».
- Live JSON-RPC на `/root/.arsumbris/au-mcp/run/adcfe8680a485529.sock` → `list-capabilities` = **49 callables** (14 rw / 32 read / 3 none).
- FNV-1a-64(`/root/vault`) = `adcfe8680a485529` (совпало) → воркспейс = `/root/vault`.
- `/root/arsumbris/*/README.md`, `arsumbris/{README,au-mcp,au-engine,au-host,INSTALL}.md`, `VERSION.md` (0.0.1-alpha, 22 части).
- `/root/.local/bin/muse-bin-1.4.9-R6297.1` → MSP v1, `tbh_*` crates, Meta Muse Code; `grep arsumbris|JEV` = 0.
- `/tmp/omp-src/packages/coding-agent/src/{priority.json,config/model-roles.ts,judgment/index.ts,judgment/cache.ts,export/ttsr.ts,export/ttsr-settings.ts}` → JEV как `judge`-роль + TTSR.
- EXA: arsumbris.ai (Volvelle Labs), docs.typesafe.ai (System One / Jev / coding-agents), jev-mcp серверы, MLflow `make_judge(typesafe:/)`.
