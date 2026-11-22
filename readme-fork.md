# readme-fork — allspark fork functionality

This file documents everything the **allspark fork** adds on top of upstream
`prime-agent`. Everything listed here is marked `FORK-ONLY` in code and is
**not** meant to be pushed upstream. Keep new fork work out of upstream files
where possible (new crates / new files), so merges stay conflict-free.

## 1. Identity model (read first)

The fork runs as **allspark**; the compiled payload binary deliberately keeps
the upstream name **prime-agent**. See the "Fork identity and isolation
(allspark overlay)" section in `AGENTS.md` for the rules. Short version:

- user-facing entry: the `allspark` launcher (`scripts/allspark-launcher.sh`,
  deployed as `bin/allspark` with the payload at `libexec/prime-agent`);
- isolation is structural: `HOME` → `~/.allspark-home`
  (`ALLSPARK_HOME` overrides), `PRIME_AGENT_DAEMON_SOCKET` pinned to
  `/tmp/allspark-daemon.sock`, `PRIME_AGENT_CODING_AGENT_DIR` pinned into the
  isolated home;
- **no seeding**: the fork never reads or copies anything from
  `$HOME/.prime` (upstream providers/models/auth must not leak in);
- upstream env-var names are kept on purpose; the launcher pins the three
  state paths so ambient upstream values cannot redirect the fork;
- `--version` prints `allspark <version> (fork build <id>; upstream
  prime-agent <version>)`; the brand splash shows `v<version>-<fork build id>`
  (e.g. `v0.10.0-fork.3`).

## 2. Fork versioning

Stamp every release build with the fork build id matching the release tag
`allspark-v<version>-fork.NNN` (zero-padded three-digit fork counter: `fork.001`, `fork.002`, `fork.003`, ...): every release documents itself in [`CHANGELOG-FORK.md`](CHANGELOG-FORK.md), which is also the source for the GitHub release notes.

```sh
ALLSPARK_FORK_BUILD=fork.NNN cargo build --profile dist --bin prime-agent --locked
```

Unstamped builds report `dev`. Two binaries are told apart by this id.

## 3. New crates (all `#![forbid(unsafe_code)]`, pedantic-clean)

| Crate | Tests | What it is |
|---|---|---|
| `crates/pa-lsp` | 158 | LSP client (async-lsp), 6-server table (rust-analyzer, typescript, gopls, pyright, clangd, bash), name_path addressing, symbol layer (overview/find_symbol/references/implementations/replace_symbol_body/safe_delete), SymbolBody, writethrough, inline/deferred diagnostics split, freshness gate, workspace diagnostics; the Unix-socket mux (`mux.rs`) is built and tested but not wired into the manager |
| `crates/pa-graph` | 578 | on-disk SQLite code graph + FTS5 + BM25/RRF/MMR search, closure-repair (surface-hash), delta machinery, MinHash+LSH, 11-signal similarity, memory primitives (witness receipts, TARL ledger, ADR store, PDX layout), scoped context shards, read-only Cypher subset, Prometheus metrics, min-cut/condensation/decompiler/beam/SPANN |
| `crates/pa-edit` | 368 | the edit engine: replace/patch/apply_patch/hashline/sloppy modes, snapshot tags, seen-lines guard, no-op loop limit, streaming session (ported from pi-edit without `unsafe`). The guard machinery is complete, but nothing mints the FIRST tag: `EditStore::record_file` (the reader that would) has no production caller, so the native tool relies on the tag a rejection reports and the seen-lines guard stays dormant |
| `crates/pa-mem` | 133 | pogocache RAM-layout primitives: sharded map, Robin Hood, 48-bit ptr buckets, entry arena, spinlock, refcount snapshots, block-LZ4 snapshots, build SHA cache; quantization: int8, RaBitQ 1-bit, multi-bit, block-4-bit, matryoshka prefixes, MaxSim |
| `crates/pa-ast` | 97 | tree-sitter parsing (vendored `tree-sitter-go` for Go 1.26 `new(expr)`) plus an ast-grep structural-search layer (`ops.rs`) that is built and tested but not connected to any tool: 11 of its 13 entry points have no caller outside the crate |
| `crates/pa-diff` | 12 | jsdiff-compatible Myers diff, zero dependencies |
| `crates/pa-jev` | 27 | Jev (TypeSafe System One) decision layer: wire types for the `choice`/`score`/`noul` primitives with tolerant decode, a client with question chunking (<=4 in flight, first-failing chunk aborts siblings), retries and a 10s timeout; the `pa-core::jev` Judge/ChainJudge adds the native-only chain rule, a 5-minute cooldown on account rejections and an in-memory answer cache |

## 4. Engine integrations (fork tools in every session)

Registered in `create_session`, gated by settings (all default **on**), plus the daemon's explicit-plan tool:

| Tool | Setting | What the model gets |
|---|---|---|
| `lsp` | `lsp.enabled` | symbols, find_symbol, references, definitions, hover, diagnostics, rename, code_actions, replace/insert symbol body, safe_delete, workspace diagnostics (`file:"*"`) |
| `graph` | `graph.enabled` | index / reindex / search / trace / architecture / coverage / changes over the code graph (`graph.db` next to the agent dir) |
| `edit` | `editHybrid.enabled` | the pa-edit engine: exact replace by default, hashline/patch/apply_patch/sloppy modes, snapshot-tag + seen-lines + no-op guards, LSP writethrough on save |
| `jev` | `jev.enabled` (**default off**) | typed decisions: `ask`/`classify`/`route`/`check` actions over the calibrated decision API — `choice` (pick + probabilities), `score` (2-10 scale), `noul` (calibrated yes/no). Routes through the `[jev]` model chain; a prompted model never stands in for a failed native judge |
| `update_plan` | daemon session (always) | the session's explicit plan: replace the complete `items` list with `id`/`text`/`status` (`pending`, `in_progress`, `completed`, `blocked`). The daemon's continuation consult resumes the next unblocked item after a text-only turn; the same item is reminded at most twice without a plan update |

Extras:

- `lsp` resolves `rust-analyzer` from an installed rustup toolchain that ships
  it (`RUSTUP_HOME`, the active `$HOME/.rustup`, and the real home's `.rustup`
  above an isolated allspark home) before falling back to `PATH`. The PATH
  `rust-analyzer` is a rustup shim bound to the ACTIVE toolchain; when that
  toolchain omits the component the shim answers `Unknown binary` and the spawn
  dies as `service stopped`;
- `lsp.servers`, `lsp.idleTimeoutMs`, `graph.dbPath`, `editHybrid.defaultMode`,
  `editHybrid.allowModes` refine the surfaces;
- `jev.autoThinking = "auto"` (**default off**) lets the judge classify each turn's
  difficulty (`trivial|moderate|hard`) and set the thinking level (`low|medium|high`);
  advisory only — a missing judge, an error or an `unsure` answer leaves the configured
  level alone, and two consecutive failures suspend the consumer for the session;
- `jev.graphRerank = "auto"` (**default off**) re-ranks `graph` `search` results with one
  judgment over the top-8 candidates: hits the judge is confident do not answer the query
  are moved below the survivors, labelled `demoted-by-jev (p=…)`, and the rest are marked
  `verified-by-jev` (`details.verified_jev`). A judge failure returns the store's own
  order untouched;
- `jev.toolArbiter = "auto"` (**default `auto`**) adds a `[tool-arbiter]` note when a
  shell command or a read-only Python cell is a habit a fork tool covers better
  (`grep`/`rg`/`find`/`fd` → `graph.search` or `lsp.find_symbol`, `sed -i`/`perl -i`/`awk -i inplace`/
  `cat`/`head`/`tail` → `edit`). A cell's `open`/glob/tree walk/`subprocess` grep gets the
  script-first wording — call the specialized tool and return only the values you need;
  writes and deletes (`write_text`, `open(...,'w')`, `rmtree`) never get the Python note.
  Advisory by construction: the call runs unchanged, its exit status is untouched, and the
  note is appended after the output (`details.toolArbiter`). The deterministic table needs
  no judge; behind a native judge the arbiter asks one `choice` and a confident
  `use_specialized` strengthens the wording. `off` silences it entirely;
  `jev.toolArbiterMaxNotes`, `jev.toolArbiterTimeoutMs` and
  `jev.toolArbiterMaxCommandChars` tune the note budget, judgment bound and
  judgment-state size (defaults 3 / 2500 ms / 400);
- `[limits]` bounds how much tool output reaches the transcript and the prompt
  cache: `bashOutputBytes` / `bashOutputLines` for a USER `!` shell result
  (defaults 50 KiB / 2000 lines), `kernelBashOutputBytes` for the MODEL's
  `bash()` in the Python kernel (default 512 KiB head + 1.5 MiB tail, the
  runtime's 1:3 ratio), and `kernelOutputChars` for one `python_repl` cell
  (default 65536, with the overflow spilled and named by a marker). A value of
  `0` or an unset key keeps the compiled default;
- `[graph]` makes three graph windows settings instead of compiled
  constants: `outputMaxBytes` caps one rendered `architecture` report (default
  50 KiB, cut named in the answer), `packSymbols` is a `context` pack's
  default symbol budget (default 12, clamped to the pack ceiling 64), and
  `searchLimit` replaces the store's default for a `search` that sets no
  `limit` (clamped to the search ceiling 200). A call's own `limit` always
  wins, a value of `0` or an unset key keeps the compiled default, and a value
  above a ceiling is clamped rather than honoured — a setting cannot raise a
  bound the model itself cannot raise;
- both consumers ride the same native-only gate as the tool (`[jev] enabled` must admit
  the layer and the chain must start native) and must pass the offline corpus gate in
  `crates/pa-core/src/jev/eval/` before a default flip: frozen corpus, dev/test split,
  one test touch, accuracy-delta + p95-latency thresholds;
- when any fork tool is in the roster, a dynamic **native-tools** prompt
  section documents it **and prefers these tools over `python_repl` for
  their domains** (lsp = navigation, edit = edits, graph = structure);
  the prompt parity tests stay green because the section only renders when
  the tools are present;
- `goal.complete()` is refused while the session's `update_plan` still has
  `pending`/`in_progress` items; the model must finish them or mark them
  `blocked` first. This keeps the goal loop from closing on a partial plan.
- `[project] enabled = true` (**default off**) keeps a durable project record
  outside the session: `update_plan` appends to
  `<agentDir>/projects/<id>/events.jsonl` and the read model at `project.db` is
  projected from it after each write. The log is canonical and the model
  derived, so a lost `project.db` is rebuilt from the log. Project identity is
  the git remote URL, else the git common dir, else the working directory,
  hashed — one repository's worktrees share a record and two clones do not.
  A session that starts with no plan of its own adopts the project's stored
  plan in its recorded order; a session that already has one keeps it, because
  its own record is the obligation it is under.
  With the setting absent nothing is written, so an existing session is
  byte-identical.
  Every model plan change is also written as a `session_plan_state` custom
  entry, so the plan survives a compaction that drops the `update_plan` call;
  replay reads the newest durable record and falls back to the tool call for
  sessions written before the entry existed.
- A persistent shell (`bash(..., persistent=True)`) is a service: it does
  not hold a goal, plan, or autonomous continuation open. A finite background
  job still does. A non-persistent long-running service has no distinct signal
  yet and still counts as a job.
- `/session` renders a **Native tools:** row listing the registered fork
  tools, so the operator sees the blocks are live; it also shows the explicit
  plan's settled count and up to three unfinished items;
- `update_plan` keeps plan ids unique and stable and allows at most one
  `in_progress` item; an update that breaks either rule is rejected before the
  plan is replaced. An item may declare `verify` (what would show it done) and
  carry `evidence` (the command and result); a `completed` item that declared a
  check but recorded no evidence blocks goal completion;
- the roster passes through the tool-surface profile before the prompt is
  built (see below).

## 5. Tool-surface profiles (`toolProfile`, Serena patterns)

`crates/pa-core/src/tool_profile/` — configure the effective tool surface:

```json
{ "toolProfile": { "context": "coding", "modes": ["editing"],
                   "excludedTools": [], "includedTools": [] } }
```

- contexts (`coding`/`query`/`minimal`/`debug` presets): excluded/included/
  fixed tools, excluded APIs, description overrides;
- modes: editing, planning, one-shot, interactive, onboarding, no-memories,
  no-onboarding, benchmark, query-projects (each with a prompt addendum);
- layering order: config → context → base modes → dynamic modes →
  single-project → project (inclusion beats exclusion);
- supporting machinery: tool markers, tombstoned registry, progressive
  disclosure, anti-hijack prompt section, prompt templates
  (`{{ tool_names }}`), anti-grep hooks, serialized task executor, session
  registry (LRU + idle TTL), leak recovery, bash output minimizer (25
  filters).

## 6. Debug logging (`debugLog`) — fork observability

```json
{ "debugLog": { "enabled": true, "level": "debug",
                "maxFiles": 10, "maxBytes": 10485760 } }
```

- `ALLSPARK_DEBUG=1` force-enables regardless of the setting;
- `ALLSPARK_DEBUG_FILTER=pa_ai=debug,pa_daemon=trace` switches to the full
  directive grammar (per-module control);
- one file per process component under `<agentDir>/logs/`:
  `debug-daemon.log`, `debug-worker.log`, `debug-cli.log` — the daemon/worker
  launches install their own component and never claim `cli` (a second
  process-global subscriber is rejected, so the components cannot collapse
  into one file);
- size-based rotation: `debug.log` → `.1` → `.N` (oldest dropped);
- includes provider HTTP trace (method, url, status, elapsed, transport
  error) — never headers/bodies/tokens;
- OAuth transport failures surface their underlying cause in the panel
  message too (`Muse OAuth request failed (<cause>). …`).

Existing surfaces (unchanged): daemon supervisor log
(`allspark-daemon.sock.*.log`), `worker-*.stderr.log`, the session JSONL
(every provider error with diagnostics), `requestTiming` → `logs/agent.jsonl`,
and `allspark incident --since "1 hour ago"` for an operator timeline.

### /cache — per-model cache statistics

`/cache` renders per-model cache stats over the persisted session branch:
input / cached (hit%) / write / total tokens per provider+model, the current
model marked, and a **Saved: $X.XX** estimate — cached tokens priced at the
full uncached input rate minus the actual cache-priced spend, computed from
the session's own cost breakdown (no external pricing data needed).

## 6b. Judge-chain providers (fork overlay)

| Provider | Model | What it is |
|---|---|---|
| `subquadratic` | `subq-snap-preview` | SubQ Snap — a decision model speaking the TypeSafe System One wire byte-for-byte (`POST /v1/systemone`, verified live). Joins the `[jev]` chain through the existing `typesafe` dialect with zero Rust of its own; judgments only (never a session model). $0.05/1M input, output free, caps 16 questions per request. Key: `SUBQ_API_KEY`. Live smoke: `SUBQ_LIVE=1 SUBQ_API_KEY=... cargo test -p pa-jev --test live_subq -- --ignored`. |

## 7. Providers (fork overlay)

- `codebuddy` (IDE plan, `www.codebuddy.ai/v2`, stream-only), `genspark`
  (`genspark.ai` LiteLLM), `muse` (Muse Code via Meta device flow) — device
  flows run on the inline auth panel (`/login` → provider row);
- **Muse specifics** (extracted from the official launcher + binary):
  - device flow: POST `auth.meta.com/oidc/device/authorization/` with
    `client_id=1031625952748946`, UA `muse-code/launcher-3` (no redirects,
    3xx = hard error — matches the launcher's `--max-redirs 0`);
  - token poll: POST `/oidc/device/token/` with the device code grant;
    `slow_down` bumps the interval by 5s (launcher verbatim);
  - key mint: POST `api.meta.ai/muse-code/key` with Bearer access token,
    `x-api-version: 1.0.0`, body `{"onboard": true}` — returns the durable
    `LLM|...` API key (no refresh path; re-login when it expires);
  - error taxonomy from the muse binary: `MODEL_API__ONBOARDING_REQUIRED`
    (470: finish onboarding at dev.meta.ai), "payment method required",
    "saved login no longer valid" (token expiry → re-login), standard OAuth
    errors (`access_denied`, `invalid_scope`, `unauthorized_client`);
  - gateway chat headers (api.meta.ai/v1): `x-client-id` (tbh:<surface>),
    `x-meta-ai-gateway-session-id`, `traceparent`, `x-fb-routing-control`,
    `x-api-version` — these are on the roadmap for the muse provider's chat
    transport (not needed for the login flow).
- the fork's own model catalog has three layers: `crates/pa-ai/assets/fork-models.v1.json`
  (the shipped catalog, compiled into the binary — the source of what the build
  serves, so a new fork model is an asset edit, not a Rust edit), then
  `$AGENT_DIR/models/fork-models.v1.json` (operator-editable, read on every load —
  adding or overriding a fork model needs no rebuild; the schema is the
  `models.json` one), then `models.json` (highest precedence on a `(provider, id)`
  conflict) in the fork config dir (never seeded from upstream);
- OAuth refresh + failover behave per provider; transport errors now surface
  their underlying cause (DNS/TLS/reset) in the panel message;
- the Muse device flow sends the canonical launcher identity
  (`User-Agent: muse-code/launcher-3`, extracted from the official
  launcher script) — without it Meta serves an HTML interstitial instead
  of the JSON API; all three muse requests follow redirects (the edge
  intermittently 302s them).

## 7b. Hardening shipped 2026-10-16 (review wave)

- **Provider safety**: in-stream credit-exhaustion frames classify as
  `payment_required` (the failover reroute fires instead of a retry ladder);
  every provider request carries a bounded timeout even when a cancel signal
  is attached.
- **Installer/update**: env-controlled installer and download URLs are
  https-only (`PRIME_AGENT_ALLOW_HTTP` opts out, mirroring the shell
  installers); the installer stop ladder knows the fork socket and never
  probes or stops the upstream TS daemon from a fork run (and vice versa).
- **Kernel**: error payloads (`ename`/`evalue`/`traceback`) clip at the
  settle cap like result text; snapshot restore neutralizes write-mode file
  handles instead of truncating user files on revive (upstream #3447 class).
- **UI critical path**: compaction transcript rebuilds and stats refreshes
  run as spawned epoch-guarded tasks; telemetry callsites never await on the
  run loop.
- **Input caps**: `@file` text ingestion caps at 256 KiB with an explicit
  truncation marker; compaction's token estimate counts image geometry
  (`ceil(w*h/750)` via `pa-ai`) instead of zero for user images.
- **OAuth**: the Anthropic login mints an independent OAuth `state` (the
  PKCE verifier never rides the authorize URL).
- See `docs/fork-identity.md` for the isolation model contract and the
  known deviations (Unix runtime dir, shared Windows pipe).

## 7c-pre. Quick start with a free LLM (9router)

The zero-cost path: run [9router](https://github.com/decolua/9router)
(a Node.js proxy that routes to OpenCode's free tier, OpenRouter `:free`
models, and other free providers), point allspark at it, and every
`oc/`-prefixed model is free:

```bash
# 1. Run 9router (docker or npm); set an API key in its dashboard
# 2. Point allspark at it:
export NINEROUTER_URL=http://localhost:20128/v1
export NINEROUTER_API_KEY=<your-9router-key>

# 3. Use a free model:
allspark -m ninerouter/oc/deepseek-v4.1-flash-free
```

The 9router process handles the client fingerprint OpenCode requires
(free tier blocks non-Node.js HTTP stacks); allspark speaks standard
OpenAI chat completions to it. The catalog carries three free models
(`oc/deepseek-v4.1-flash-free`, `oc/glm-5.3-flash-free`,
`oc/muse-spark-1.3-contributor-free`); fetch the full live list from
`GET $NINEROUTER_URL/models` and add them to `models.json` as needed.

## 7c. Diagnostic profiling (`--features profiling`, fork)

The supervisor's admission plane carries opt-in hotpath instrumentation:

```bash
ALLSPARK_FORK_BUILD=dev cargo build -p pa-daemon --features profiling
# report lands in <agent-dir>/logs/hotpath-report.txt at supervisor exit
```

Default builds pull zero new packages (the dep is optional); the feature
costs ~200ns per instrumented call, so it never rides release profiles.
The instrumentation uses only the runtime-label macro arm — hotpath's
literal-label forms generate `unsafe(export_name)` statics that this
crate's `forbid(unsafe_code)` rejects (labels are block-wrapped to force
the expression arm). The `hotpath_init`/`hotpath_init_ci` agent skills
are installed for instrumentation sweeps, and hotpath's own MCP server
(hotpath's `hotpath-mcp` feature, run as a separate tool) can join the
fork's MCP config for agent-driven report queries.

## 8. Conventions

- every fork crate/file carries a `FORK-ONLY` marker; new fork work goes into
  new crates/files first;
- wire identifiers (protocol names, `_meta` namespaces, `PI_PACKAGE_DIR`,
  provider ids) stay byte-compatible with upstream — PARITY BEATS BRANDING ON
  THE WIRE;
- user-visible product strings in the fork surfaces say **allspark**; the
  payload binary name and `pa-*` crate names stay upstream;
- the in-binary rebrand branches (`feat/allspark-identity`,
  `feat/allspark-identity-reland`) are the **abandoned** alternative — keep
  them unmerged.

## 9. Operations quick reference

```sh
# stamped release build
ALLSPARK_FORK_BUILD=fork.NNN cargo build --profile dist --bin prime-agent --locked

# deploy layout
bin/allspark                 (launcher script, tracked)
libexec/prime-agent          (payload binary)

# verify a release artifact AND the installer, without touching a running daemon
# (AGENTS.md forbids running install-rust.sh from inside a session: it stops
#  daemons by socket path, so an experiment is indistinguishable from a real
#  update. A container has its own /tmp and PID namespace, so it cannot reach
#  one. `--archive` installs a local tarball and needs no network; the image
#  needs python3, because the installer falls back to a system Python when it
#  cannot bootstrap uv.)
docker build -t allspark-verify - <<'EOF'
FROM ubuntu:24.04
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends python3 ca-certificates
EOF
docker run --rm --network none \
  -v "$PWD/dist:/dist:ro" -v "$PWD/install-rust.sh:/install-rust.sh:ro" allspark-verify \
  sh -c 'PRIME_AGENT_RUST_PREFIX=/opt/allspark HOME=/root \
         sh /install-rust.sh --archive /dist/<archive>.tar.gz --force --verbose \
         && /opt/allspark/bin/allspark --version'
# what a pass proves: bin/allspark + libexec/ payload + the bundled catalog,
# runtime and skills land in the fork layout; the upstream share/ and bin/
# paths stay empty; --version carries the fork build stamp; the launcher
# honours ALLSPARK_HOME and creates its socket directory 0700; a second run
# over the same prefix replaces the payload cleanly.

# apply settings changes to a live session without restarting the daemon:
# the TUI `/reload` reloads MCP, retires the built session and lets the next
# prompt rebuild the tool roster/prompt from the files; the Python namespace is
# snapshotted. Cold settings (socket, session dir, transport, kernel venv) still
# need a daemon restart.
# models added to <agentDir>/models/fork-models.v1.json are picked up by the
# same reload (no rebuild).

# logs / diagnostics
<agentDir>/logs/             daemon log, worker stderr, debug-*.log, agent.jsonl
allspark incident --since "2 hours ago"
allspark --version           # allspark <version> (fork build fork.NNN; upstream prime-agent <version>)
```
