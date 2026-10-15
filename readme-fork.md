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
`allspark-v<version>-fork.N`:

```sh
ALLSPARK_FORK_BUILD=fork.N cargo build --profile dist --bin prime-agent --locked
```

Unstamped builds report `dev`. Two binaries are told apart by this id.

## 3. New crates (all `#![forbid(unsafe_code)]`, pedantic-clean)

| Crate | Tests | What it is |
|---|---|---|
| `crates/pa-lsp` | 143 | LSP client (async-lsp), 57-server table, name_path addressing, symbol layer (overview/find_symbol/references/implementations/replace_symbol_body/safe_delete), SymbolBody, writethrough, inline/deferred diagnostics split, freshness gate, workspace diagnostics, Unix-socket mux |
| `crates/pa-graph` | 466 | on-disk SQLite code graph + FTS5 + BM25/RRF/MMR search, closure-repair (surface-hash), delta machinery, MinHash+LSH, 11-signal similarity, memory primitives (witness receipts, TARL ledger, ADR store, PDX layout), scoped context shards, read-only Cypher subset, Prometheus metrics, min-cut/condensation/decompiler/beam/SPANN |
| `crates/pa-edit` | 349 | the edit engine: replace/patch/apply_patch/hashline/sloppy modes, snapshot tags, seen-lines guard, no-op loop limit, streaming session (ported from pi-edit without `unsafe`) |
| `crates/pa-mem` | 126 | pogocache RAM-layout primitives: sharded map, Robin Hood, 48-bit ptr buckets, entry arena, spinlock, refcount snapshots, block-LZ4 snapshots, build SHA cache; quantization: int8, RaBitQ 1-bit, multi-bit, block-4-bit, matryoshka prefixes, MaxSim |
| `crates/pa-ast` | 97 | tree-sitter + ast-grep parsing layer (vendored `tree-sitter-go` for Go 1.26 `new(expr)`) |
| `crates/pa-diff` | 8 | jsdiff-compatible Myers diff, zero dependencies |

## 4. Engine integrations (fork tools in every session)

Registered in `create_session`, gated by settings (all default **on**):

| Tool | Setting | What the model gets |
|---|---|---|
| `lsp` | `lsp.enabled` | symbols, find_symbol, references, definitions, hover, diagnostics, rename, code_actions, replace/insert symbol body, safe_delete, workspace diagnostics (`file:"*"`) |
| `graph` | `graph.enabled` | index / reindex / search / trace / architecture / coverage / changes over the code graph (`graph.db` next to the agent dir) |
| `edit` | `editHybrid.enabled` | the pa-edit engine: exact replace by default, hashline/patch/apply_patch/sloppy modes, snapshot-tag + seen-lines + no-op guards, LSP writethrough on save |

Extras:

- `lsp.servers`, `lsp.idleTimeoutMs`, `graph.dbPath`, `editHybrid.defaultMode`,
  `editHybrid.allowModes` refine the surfaces;
- when any fork tool is in the roster, a dynamic **native-tools** prompt
  section documents it (the prompt parity tests stay green because the
  section only renders when the tools are present);
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
- per-process files under `<agentDir>/logs/`: `debug-daemon.log`,
  `debug-worker.log`, `debug-cli.log`;
- size-based rotation: `debug.log` → `.1` → `.N` (oldest dropped);
- includes provider HTTP trace (method, host, status, elapsed, transport
  error) — never headers/bodies/tokens.

Existing logs (unchanged): daemon supervisor log
(`allspark-daemon.sock.*.log`), `worker-*.stderr.log`, the session JSONL
(every provider error with diagnostics), `requestTiming` → `logs/agent.jsonl`,
and `allspark incident --since "1 hour ago"` for an operator timeline.

## 7. Providers (fork overlay)

- `codebuddy` (IDE plan, `www.codebuddy.ai/v2`, stream-only), `genspark`
  (`genspark.ai` LiteLLM), `muse` (Muse Code via Meta device flow) — device
  flows run on the inline auth panel (`/login` → provider row);
- the fork's own model catalog lives in `models_fork.rs` (compiled) and can
  be overridden by `models.json` in the fork config dir (never seeded from
  upstream);
- OAuth refresh + failover behave per provider; transport errors now surface
  their underlying cause (DNS/TLS/reset) in the panel message.

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
ALLSPARK_FORK_BUILD=fork.N cargo build --profile dist --bin prime-agent --locked

# deploy layout
bin/allspark                 (launcher script, tracked)
libexec/prime-agent          (payload binary)

# logs / diagnostics
<agentDir>/logs/             daemon log, worker stderr, debug-*.log, agent.jsonl
allspark incident --since "2 hours ago"
allspark --version           # allspark <version> (fork build fork.N; upstream prime-agent <version>)
```
