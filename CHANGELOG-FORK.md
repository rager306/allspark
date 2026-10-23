<!-- FORK-ONLY: upstream merge must not overwrite this file -->
# Fork changelog (allspark)

One entry per fork release, newest first. The fork counter is zero-padded
three digits (`fork.NNN`); the tag is `allspark-v<version>-fork.NNN` and the
binary's `--version` carries the same id via `ALLSPARK_FORK_BUILD` at build
time. Update this file in the same change that cuts a release; the GitHub
release notes for a version are generated from its entry here.

Pre-numbering history: three single-digit tags, all superseded by the NNN scheme
below. `fork.1` and `fork.2` (2026-10-13) marked the abandoned in-binary rebrand
branch (`feat/allspark-identity`). `fork.3` points at the SAME commit as `fork.2`
(`375ad95cf`) and is a duplicate tag, not a separate release — it has no entry here
because nothing shipped between them. `FORK.md` carries the same correction.

## fork.010 — upstream v0.10.2-beta.3

- Tag: `allspark-v0.10.2-beta.3-fork.010` (2026-10-23)
- Base: `5a3df6fc9` (fork.009) — the first fork release built on an upstream
  NIGHTLY rather than a stable tag: `v0.10.2-beta.3` (`9eb29e29b`) is a
  tag-only snapshot of upstream `main`, 27 commits past `v0.10.1`.

**Upstream merge (26 commits).** Measured before starting: 17 real content
conflicts out of 1 415 files this fork has changed, and zero in the fork-only
crates (`pa-graph`, `pa-lsp`, `pa-ast`, `pa-edit`, `pa-mem`, `pa-jev`,
`pa-diff`). Four of these fixes close the highest-priority findings of the
2026-10-23 architecture audit, which had recorded them as still open in the
fork:

- **`dd384e3a2`** — a spawned child is admitted only after its task prompt
  lands (#3708). Previously `prompt_admitted` was set before `prompt_child`
  finished, so a parallel collect could read the idle pre-prompt worker as
  done and declare a task complete with no answer.
- **`71beca947`** — child cancellation records a semantic parent→child edge
  and restores live status when the abort fails (#3680). The registry used to
  publish `cancelled` and ignore the Abort result.
- **`c20f93f67`** — print mode wires background bash, heartbeats, skills and
  late receipts like the daemon (#3580). Headless used to pass `None` for the
  cron store and completion callbacks, then fabricate a private store.
- **`00ec2549e`** — compaction summaries preserve outstanding orchestration
  state (#3685), so a compaction no longer loses pending verifiers or
  background handles.

Also merged: persistent `bash()` shells that survive kernel and daemon
restarts (#3640); a per-cell `python_repl` watchdog (#3691); TUI
escape-sequence safety and reader-position preservation (#3698); the stale
"await commands before ending the turn" prompt rule dropped (#3511); ACP
transport-failure classification (#3679); transcript page locking (#3705);
the session archive removed (#3657); the webpki-roots RUSTSEC-2026-0381
deny cleared (#3694, which independently made the same change the fork had
already documented as its reason).

**How the conflicts were resolved.** Settings (`types`, `mod`, `load`,
`manager`) took a union: upstream's `pythonCellTimeoutMs` plus the fork's
`graph`, `toolProfile`, `jev`, `editHybrid`, `lsp`, `agents`, `debugLog`,
`routing`. The session-archive constants went with the archive. The engine
took the fork's native-tool gates plus upstream's per-cell timeout.
`engine/tests.rs` is upstream's file with the fork's five unique tests
appended. The kernel readiness probe merged segment-by-segment into 57
checks (upstream's persistent-shell assertions plus the fork's
`pid`/`command` fields). The CI lifecycle leg took upstream's deletion over
the fork's `if: false`. `Cargo.lock` was regenerated from the fork's lock so
the fork-only crates stay in it.

**Fork fixes in the same release:**

- `c47178fdc` — a model-resolution fixture no longer inherits ambient
  provider keys. Upstream #3628's test asserts a fallback to the fixture's
  own model; on a machine exporting `GOOGLE_*`/`ANTHROPIC_*`, five providers
  looked credentialed and the fixture's model ranked fifth.
- `6a7ab294e` — the gate's zero-test skip survives nextest's colour (it
  colourises each word, splitting the marker), and the `websearch` core
  bullet upstream #3677 deleted stays deleted.
- `1bf52d878` — the gate builds every test target once instead of 229 times.
  Measured: 1540s of 3232s was per-unit `cargo` freshness checks. Now 1519s.
  `--lib --tests --bins`, not `--all-targets`: examples and benches are
  never run, and linking them exhausted the disk.
- `360ba7f0b` — the native `edit` tool declares a sequential execution
  constraint. An edit is a read→stage→write cycle and the loop dispatches tools
  in parallel by default, so two edits in one batch both staged from the same
  version and the second write discarded the first; a per-file write queue does
  not close it, because by then both reads have happened.
- `9f47dc612` — two roots with the same directory name no longer share one
  graph project. The name is the root's last path component and `projects.name`
  is the primary key, so registration moved the row to the second root while the
  first root's nodes stayed behind as orphans: a query from the first root
  answered `fresh` with none of its symbols. Registration now refuses and names
  both paths; a symlinked spelling of the same tree still registers.

## fork.009 — upstream v0.10.1-beta.38

- Tag: `allspark-v0.10.1-beta.38-fork.009` (2026-10-20)
- Base: `cf206108b` (fork.008) — 69 commits: the beta.38 upstream merge,
  the fork CI and nightly cleanup, the toolchain pin, and the wasmtime
  advisory bump.

**Upstream merge (beta.35 -> beta.38, 12 commits)**: the windowed
transcript pager (#3674 — scrolling now continues past a compaction into
pre-compaction history, and a compaction no longer rebuilds the visible
transcript); head-and-tail clipping for `python_repl` cell output
(#3526); subagent roster and `agent_observe` fidelity (#3576); a bounded
degrade window for `await bash()` (#3523); paged saved-session listing
(#3656); read-only kernel venvs that degrade instead of raw EACCES
(#3633); empty compaction summaries rejected and length-capped ones
retried once (#3529); the stop escalation signal without pidfd (#3404).

**Fork regressions caught by the merge**:
- The kernel-error settle cap was lost when the merge took upstream's
  `execution.rs`; restored (`ename`/`evalue`/`traceback` clip through
  `clip_at_chars`).
- **Product bug fixed**: a daemon started inside a session dragged
  `PRIME_AGENT_SESSION_DIR` (and the internal worker-handshake vars)
  into every worker it spawned, so an isolated harness wrote its RLM
  ledger rows into the outer tree. `apply_worker_env` now scrubs them
  before the launch env lands.
- `import_compaction_e2e` read `{session}.window-cache.json` (new in
  #3674) instead of the transcript; it now filters on `.jsonl`.
- The fork's `loop_rpc_spawned_headless` test asserted the
  `spawn_transcript_rebuild` mechanism that #3674 replaced with the
  windowed pager; removed (upstream's `transcript_window_headless` and
  `windowed_backfill_headless` carry the coverage).

**TS parity is advisory, and its CI gate is off (fork)**: upstream's own
#3554 removed the parity scripts that could not catch regressions but
kept 59 of them, including `review_model_facts_parity.py` — whose leaked
reference daemons burned a core for 7.5 hours on the build host. The
fork disables the `lifecycle_parity` step outright (`if: false`, the body
stays) and documents parity as advisory in AGENTS.md.

**Scheduled CI noise removed**: the 10-minute `discussion-slack` cron
(the notify job gates on the upstream repository and always skipped on
the fork), the nightly `windows-runtime-triage` wave (50+ minutes of a
windows-2022 runner per day, kept failing), and the `daily-stats` cron
(gated on an unset variable). Their `workflow_dispatch` and PR waves stay.

**Toolchain pinned in-tree**: `rust-toolchain.toml` selects 1.98.1, the
same version CI pins, so local clippy/test no longer diverge from CI.

**wasmtime advisories cleared**: RustSec published six advisories
(RUSTSEC-2026-0374..0380) against `wasmtime` on 2026-10-20. `ast-grep-core`
0.39 -> 0.45.3, `tree-sitter` 0.25 -> 0.27.1, `wasmtime` 29.0.1 -> 48.0.7
(the patched series). One API break absorbed: `extract_injections` now
returns `Vec<(String, Vec<Range>)>`.

**Verification**: PR #1 green (18/18 jobs) before merge; the post-merge
`main` push run `39417228406` green (18/18, 16m42s).

## fork.008 — tool arbiter, review hardening, daemon-lane QA

- Tag: `allspark-v0.10.0-fork.008` (2026-10-18)
- Base: `37e9164ee` (fork.007) — the arbiter, the second review's P1
  fixes, the summarizer overflow guard, admission telemetry, and the QA
  daemon lane.

**The tool arbiter** (the last ADR consumer): a deterministic table of
bash-command patterns (grep, sed, find, cat) that suggests the
specialized fork tool (graph.search, edit, lsp) as an advisory note on
the bash result — never blocking, never redirecting, the omp lesson
applied. Behind `[jev] toolArbiter = auto` (the promoted default);
corpus-gated like the other consumers.

**Second review's P1s fixed**: the compaction summarizer now truncates
history to the model's context window before building its request (the
live "prompt is too long" compaction loop); the worker-side admission
link deadline outlives the supervisor-side queue budget (no more orphan
leases); the settle path checks the breaker before dispatching queued
waiters (an open breaker now actually stops admissions); the admission
token is shape-validated (not just non-empty) and lease ids start from
a random offset.

**Adoption telemetry for the admission plane**: breaker transitions,
AIMD steps, and the snapshot rollup land in pa-telemetry
(fire-and-forget through an unbounded channel).

**QA daemon-lane**: the deviation suite drives the real daemon
(JSON-RPC session management, worker kill) — RT-012 (failover) passes;
RT-006 (exactly-once) is a known-red (the daemon replays the prompt to
the restarted worker; the TUI fix covers the client path, the daemon
path is an upstream candidate).

## fork.007 — jev consumer defaults promoted to `auto`

- Tag: `allspark-v0.10.0-fork.007` (2026-10-18)
- Base: `6984252fa` — the default flip, one settings change plus the
  test that pins it.

The live promotion runs over the frozen corpora (through SubQ Snap, the
real judge) measured auto-thinking at **+50.0 points** on the test split
(0.417 → 0.917) and graph re-rank at **+8.3 points** (0.333 → 0.417).
Both consumer defaults move from `off` to `auto`: armed, but only
activating behind a resolved native judge. Without a judge (or on any
judge failure, or after two consecutive failures) the behaviour is
byte-identical to the pre-jev path — the consumers are advisory and
fail-open by construction. A settings typo still reads as `off`
(fail-closed on user error).

Also in this release: the pa-mem varint decoder refuses non-canonical
tenth bytes instead of silently truncating bits above 63, and the opt-in
hotpath profiling feature instruments the admission plane's acquire and
settle spans in diagnostic builds.

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
