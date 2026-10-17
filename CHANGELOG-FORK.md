<!-- FORK-ONLY: upstream merge must not overwrite this file -->
# Fork changelog (allspark)

One entry per fork release, newest first. The fork counter is zero-padded
three digits (`fork.NNN`); the tag is `allspark-v<version>-fork.NNN` and the
binary's `--version` carries the same id via `ALLSPARK_FORK_BUILD` at build
time. Update this file in the same change that cuts a release; the GitHub
release notes for a version are generated from its entry here.

Pre-numbering history: two single-digit tags (fork.1, fork.2, 2026-10-13)
marked the abandoned in-binary rebrand branch (`feat/allspark-identity`);
they were local-only artifacts and are superseded by the NNN scheme below.

## fork.006 — upstream v0.10.1-beta.35 merged

- Tag: `allspark-v0.10.0-fork.006` (2026-10-18)
- Base: `e4bc92bc8` — the beta.35 merge join plus the deviation suite's
  log/output expansion.

**Upstream v0.10.1-beta.35 in full** (36 commits): the perf wave (ledger
replay cache, session sidecars at worker exit, once-per-process trust
store, TUI subagent dock driven from events), shutdown scoped to the
owning daemon tree, markdown TS v0.9.8 rendering parity, factory worker
lifecycle (respawn-once, dead-child reaping), kernel model facts rebound
on daemon switches, @-token completions, MCP clientId/callbackPort, the
slack connector, and the perf CI corpus.

**Conflict resolutions kept both semantics**: the installer gains
upstream's sun_path byte-limit fallback alongside the fork socket (the
same rule applied to both), worker launch routes on upstream's strict
subcommand check while keeping the fork's early detection for the
debug-log seam, daemon discovery keeps the cross-product guard inside
upstream's Option socket-dir model (both suites' tests green), compaction
keeps the geometry-aware image tokens under upstream's retention-floored
threshold. Upstream's new agent-engine tests learn the fork's admission
field, and its default-rooted shutdown test runs under the fork's
default socket-dir name — the one real collision (the test parked a
daemon on the upstream product-default path, which the fork's
cross-product guard refuses by design) resolved in the test, not by
weakening the guard.

**The deviation suite reads logs and output now**: a per-trial log layer
(session transcript timeline, request-timing stream, the rotating debug
log) with log invariants, an output oracle over the agent's answer, and
a live judge tier — SubQ Snap grades each task against its rubric and
manifests carry verdicts with probabilities, rubric hashes and judge
bundle identity. First runs caught an inverted regex-bounds default;
three tasks validated end to end.

## fork.005 — judge consumers, admission maturity, deviation suite

- Tag: `allspark-v0.10.0-fork.005` (2026-10-17)
- Base: `d473a9c2a` (fork.004) — seven commits.

**The judge layer grows its first consumers and a second backend.** SubQ
Snap joins the `[jev]` chain as a pure catalog entry (it speaks the System
One wire byte-for-byte; the live smoke ran green against the real endpoint
and its 100M free input tokens make it the default test judge). Two
consumers ship behind their own gates, both default off and advisory-only:
auto-thinking classifies each turn's difficulty with one question and sets
the thinking level; graph re-rank filters the top-8 search candidates with
one judgment, demoting confident non-answers with explicit labels. The
promotion discipline from the RuFlo lesson is structural (frozen corpora
with fingerprints, dev/test splits, one test touch, near-tie rejection) —
scripted-judge runs promote both consumers; flipping the defaults requires
a live-judge run.

**Provider admission matures (M3-M5).** Retry policy classifies
rate-limit reasons (EN/RU/CN shapes) with reason-specific waits, honours
Retry-After as a minimum with a cap, and gets 5-7 attempts on 429 ladders
instead of the three that killed subagents; quota and spend classes reroute
immediately. The supervisor registry gains per-provider retry budgets and
a circuit breaker whose open state fails stream admission fast into
failover, and a socket-level snapshot exposes limits, streaks, queues and
breaker state. The release gate caught the M3 delay silently overrunning
`retry.provider.maxRetryDelayMs` on long hints: the budget contract now
wins (a hint above the policy cap settles into the failover seam), hints
inside an admitting budget keep their minimum semantics to the 60s
ceiling.

**Real-task deviation suite (black-box levels 1+2).** Twelve seed tasks
from this fork's real failures with state oracles and trace invariants; a
stdlib runner with per-trial sandboxing, secret canaries, mock providers,
pass^N grading, behavioral fingerprints and a baseline delta gate. Its
first catch: the launcher did not pin `PRIME_AGENT_SESSION_DIR` — fixed
and green two-for-two.

## fork.004 — provider admission: the cross-session rate-limit plane

- Tag: `allspark-v0.10.0-fork.004` (2026-10-17)
- Base: `5fd98cfad` (fork.003).

The day fork.003 was cut, nine parallel subagents on one provider exhausted
its rate limit — three died retrying a ladder that could never succeed and
three more hung in streams nobody timed out. This release gives the daemon
the missing shared plane (ADR `adr-ratelimit-admission`, M1+M2):

- **AIMD per (provider, model)** in the supervisor — one registry across
  all session workers (a per-worker limit bounds nothing). Start 6,
  floor 1, ceiling from `[agents] maxConcurrentPerProvider`; a successful
  stream grows the limit back toward the ceiling, a 429 halves it.
- **Narrow bracket**: a slot is held for one LLM stream's own lifetime
  (acquire before the provider call, settle on outcome) — never for a
  worker's lifetime, so a parent streaming never deadlocks its child's
  spawn (oh-my-pi #3749's lesson, encoded as a test).
- **Spawn queue**: a spawn whose provider is at its limit parks in the
  daemon's FIFO and dispatches when a slot frees, behind a randomized
  0-2s stagger (thundering-herd guard). The admission rides a new
  token-authenticated worker-to-supervisor wire command; standalone
  workers admit everything.
- **Hygiene**: dead holders' leases are reaped; an expired wait budget
  fails open instead of wedging the queue slot; the settle outcome is
  classified from the assistant message itself.

Tests: 14 admission units (AIMD halving/growth, FIFO dispatch of nine
behind six, bracket lifetime, nested-spawn admission, stagger range,
reaping, fail-open) — all deterministic, no sleeps.

## fork.003 — hardening wave + jev decision layer

- Tag: `allspark-v0.10.0-fork.003` (2026-10-17)
- Base: `375ad95cf` (fork.002) — 17 commits, full gate green
  (fmt + clippy workspace, 6908 tests, release build).

A compound-engineering review of the whole tree (52 findings) was verified
finding-by-finding against the code; every confirmed issue below shipped with
a red-first regression test, and an independent re-verify pass re-ran each
test with the fix mutated away.

**Correctness and reliability**
- A crashed session worker no longer resurrects a consumed prompt as a clean
  draft: the worker-socket-closed refusal is classified as an ambiguous
  outcome, so a restart can't silently duplicate a partially-executed turn.
- In-stream credit-exhaustion frames (gateways fronting Anthropic) classify
  as `payment_required` — the failover chain reroutes instead of burning a
  retry ladder that can never succeed.
- Every provider request carries a bounded timeout even when a cancel signal
  is attached (the old send path discarded `timeout_ms` under a signal, and
  session turns passed none at all — hung streams waited forever).
- Compaction transcript rebuilds and stats refreshes moved off the UI run
  loop into spawned epoch-guarded tasks; telemetry callsites no longer await
  on the paint path, and the counters stop rebuilding the settings manager
  per call.

**Safety**
- Installer and download URLs from the environment are https-only (the same
  `PRIME_AGENT_ALLOW_HTTP` hatch the shell installers have), checked before
  any fetch. The installer stop ladder knows the fork socket and a fork run
  never probes or stops the upstream TS daemon (and vice versa).
- The Anthropic OAuth login mints an independent `state`; the PKCE verifier
  never rides the authorize URL (verified against the real client's
  behaviour before fixing).
- Snapshot restore neutralizes write-mode file handles instead of
  re-opening them with `O_TRUNC` — restoring a session no longer empties
  user files (the upstream #3447 class; patch prepared for upstream).
- `@file` text ingestion caps at 256 KiB with an explicit truncation marker;
  compaction's token estimate counts image geometry instead of zero for
  user images.

**New: the jev decision layer (phase 1, default off)**
- `crates/pa-jev`: System One wire types (choice/score/noul) with tolerant
  decode, chunked questions (<=4 in flight, first failure aborts siblings),
  retries with retry-after, bounded timeout.
- `pa-core::jev`: the `Judge` chain — native-only after the first native
  candidate (a prompted model never impersonates a calibrated judge), a
  5-minute cooldown on account rejections, an in-memory answer cache — plus
  the `jev` tool (`ask`/`classify`/`route`/`check`) wired behind
  `jev.enabled` (default off).

**Tests and infrastructure**
- e2e harnesses isolate ambient `PRIME_AGENT_*` overrides (running the suite
  from inside a live agent session no longer redirects sandbox daemons to
  the real install); the TS-differential lanes share one skip contract.
- The golden bash corpus pins its git dates to explicit UTC offsets —
  replay hashes no longer depend on the host timezone.
- Fork release numbering moves to zero-padded `fork.NNN`.

> **Erratum (2026-10-17):** the numbering commit carried three in-progress
> files of the next release's provider-admission work (an unregistered
> module source plus settings plumbing) past an overly-broad `git add`.
> The stray module was outside the crate's module tree, so the compiled
> `fork.003` artifact is byte-identical to the audited code above; the
> admission layer itself ships complete in fork.004.

## fork.002 — engine integrations and providers, live

- Tag: `allspark-v0.10.0-fork.002` (2026-10-16)
- Base: `51873e826` (fork.001) — 16 commits.

- The Muse OAuth chain fixed end-to-end against the official client's
  behaviour (canonical device-flow identity, redirect-following, transport
  errors surfaced): all three fork providers (CodeBuddy, Genspark, Muse)
  log in and serve.
- Native-tools visibility: the roster lists `lsp`/`graph`/`edit`, and
  `/cache` reports per-model cache-hit statistics.
- `debugLog` setting: rotating debug logs for the fork's components.
- MCP conformance and PTY multi-client exactly-once behaviour; MCP server
  names flow from settings into the worker session.
- `readme-fork.md` becomes the fork's documentation anchor (crate markers,
  component attribution, settings reference), and the fork-tools e2e drives
  the real binary through a real tool call.

## fork.001 — fork foundations: crates, identity, isolation

- Tag: `allspark-v0.10.0-fork.001` (2026-10-15)
- Base: upstream `main` merge `1d334719b`.

**Six new crates** (all `#![forbid(unsafe_code)]`):
- `pa-lsp` — async LSP client, 57-server table, symbol layer, writethrough,
  diagnostics split, Unix-socket mux.
- `pa-graph` — on-disk SQLite code graph with FTS5/BM25/RRF/MMR search,
  delta machinery, MinHash+LSH similarity, memory primitives, scoped
  context shards, read-only Cypher subset.
- `pa-edit` — the edit engine: replace/patch/apply_patch/hashline/sloppy
  modes with snapshot-tag, seen-lines and no-op guards.
- `pa-mem` — pogocache RAM-layout primitives (sharded map, Robin Hood,
  block-LZ4 snapshots; int8/RaBitQ/multi-bit/matryoshka quantization).
- `pa-ast` — tree-sitter + ast-grep parsing layer.
- `pa-diff` — jsdiff-compatible Myers diff, zero dependencies.

**Identity and isolation** (the model AGENTS.md pins):
- The `allspark` launcher: `HOME` pin plus agent-dir and daemon-socket
  overrides — a co-installed upstream shares nothing with the fork.
- No seeding from the upstream install (providers, settings, auth); the
  state dir is pinned against ambient upstream overrides.
- `--version` carries the fork build id (`allspark <v> (fork build <id>;
  upstream prime-agent <v>)`), the splash shows it, and launcher e2e tests
  drive the real binary.
