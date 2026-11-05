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

## fork.016 — the fork layout keeps its ownership marker

- Tag: `allspark-v0.10.3-beta.15-fork.016` (2026-11-05)
- Base: no upstream change; a fork-only installer fix on top of `fork.015`.

### What was broken

A fresh install of `fork.015` worked, and every `allspark update` after it failed:

```
install-rust.sh: refusing to take ownership of …/libexec: it is neither this
installer's marked payload tree (.prime-agent-install) nor the TypeScript
installer's managed root; move it aside and re-run
```

The ownership marker was written to the STAGING root, while the publish renames
`$stage/libexec` to `$PREFIX/libexec` in the fork layout. The marker therefore never
reached the published tree, and the next install refused to adopt its own previous
payload. The upstream layout hid the bug: there `payload_root == stage`.

### Fix

The marker now rides `$payload_root`, the directory the publish actually renames, so
it lands beside the payload in both layouts.

Verified end-to-end: fresh install → marker present in `libexec/` → `allspark update`
completes ("updated to the latest build"). `installer_platform_map` 28 passed;
`the_ownership_marker_rides_the_published_tree` is red-first (writing the marker to
`$stage` fails it).

## fork.015 — upstream v0.10.3-beta.15

- Tag: `allspark-v0.10.3-beta.15-fork.015` (2026-11-03)
- Base: the `v0.10.3-beta.15` upstream merge (2 conflicts resolved), 3 upstream
  commits since the `v0.10.3-beta.13` merge (`e3a3d64b6`).
- No FORK-ONLY crate was touched: the delta is two factory-runtime fixes and their
  `.changes` fragments.

### What the fork gained

- **A design node's output is no longer cut at 8 KiB.** The settle-capture cap is
  now the host's own collect bound (65,536 characters). A valid fenced JSON longer
  than the old cap was truncated mid-object, and the producing state failed with
  "no JSON object containing output" although the child had settled correctly.
- **The answer scrape accepts a bare terminal JSON object**, or one followed by a
  dirty tail, not only a fenced ```json block.

### Fork work that survived the merge unchanged

- `cancel_failures` on a run, and the restart limitation paragraph: a refused
  `delete` no longer lets a "stopped" run hide a live child, and the docstring says
  outright that a run is a kernel-lifetime object.
- The audit F11 test keeps its shape (`test_failed_delete_does_not_claim_the_child_is_cancelled`),
  and upstream's 151 new test lines landed beside it.

### The fork's install and update path works for the first time

Every earlier fork release shipped an installer that installed **upstream** Prime
Agent, and an `allspark update` that 404'd before it could install anything. Four
artifacts each assumed a different shape of one channel, and nothing compared them:

- the launcher pinned the download base to a GitHub release;
- the release published a FLAT, STABLE-ONLY asset set — no installer script, and no
  `beta` / `beta.json`, although every fork version is `-beta.N` and therefore
  NIGHTLY;
- the updater derives `<base>/install-beta.sh`;
- the installer derives `<base>/releases/v<version>/` — a path a GitHub release
  cannot express, because its tag is the only path segment it has.

Fixed on all four sides:

- **The checked-in installer names this repository.** `DOWNLOAD_BASE_URL_DEFAULT`
  and `INSTALL_LAYOUT_DEFAULT` now point at the fork's own channel and layout. The
  repo copy is what `raw.githubusercontent.com` serves, so leaving upstream's values
  there meant the documented one-liner installed upstream from the fork's repo.
- **The release publishes the channel**: `install.sh` / `install-beta.sh` rendered
  from this repo's own installer (stamps asserted, `sh -n` checked), plus `beta` and
  `beta.json` beside `stable` and `latest.json`.
- **The installer reads a flat release root**, keeping the R2 versioned prefix as
  the first try so upstream installs stay byte-identical.
- **The layout is a stamp**, not a guess from the URL: `INSTALL_LAYOUT_DEFAULT=fork`,
  so the fork payload lands in `libexec/` + `bin/allspark` and never overwrites a
  co-installed upstream install's `share/prime-agent`.

Four regression tests pin the seams, each red-first proven by reverting its half.
The layout test runs the script's own layout block in five environments — a
`contains(...)` version stayed green with the arm disabled and was rewritten.

### Verification

- `prime-agent-runtime` `test_factory.py` 321 passed, 2 subtests.
- `cargo check --workspace --all-targets` clean.
- `pa-cli` `installer_platform_map` 26 passed; the fork channel was exercised
  end-to-end against a local HTTP channel (real `releases/v<version>/` tree plus a
  flat root): beta pointer → flat fallback → sha cross-check → extraction.

## fork.014 — upstream v0.10.3-beta.13

- Tag: `allspark-v0.10.3-beta.13-fork.014` (2026-11-03)
- Base: the `v0.10.3-beta.13` upstream merge (11 conflicts resolved), 6 upstream
  commits since the `v0.10.3-beta.10` merge (`6f6680681`).
- No FORK-ONLY crate was touched: the upstream delta has zero files under
  `crates/pa-{graph,edit,lsp,ast,mem,jev,diff}` and zero fork docs/scripts.

### What the fork gained

- **`rlm.session.messages`** — the model reads its own transcript slice from the
  REPL. The trait method is implemented for the fork's inline print host, with the
  daemon's role filter and negative-`start` contract.
- **`python_repl` cells interrupt at their time limit and say why**, with a
  per-call `timeout` that replaces the session default.
- **Billed-call accounting**: refinement, review, side-question, and uncommitted
  compaction summaries now land as `model_usage` rows, and goal budgets, session
  stats, and telemetry count them.

### What the fork fixed while merging

- **A second compaction on a session that just compacted.** `auto_compaction_due`
  did not fence a STALE usage anchor the way `threshold_compaction_due` already
  did, and the stale path also re-added the system prompt + tool schemas. Measured:
  12 012 message tokens + 21 131 prefix against a ~16 000 threshold, so the turn
  loop's post-turn `run_auto_compaction` compacted again and its summarizer ate the
  next turn. A stale anchor is now dropped from the estimate while `anchored` stays
  true, which is the rule this method's doc already promised.
- **The degraded kernel stub covers `rlm.session`**, and the guard now accepts a
  nested namespace bound as `rlm.<name> = …` (the shape the real namespace has).

### Verification

- `pa-core` 1733 lib tests, `pa-daemon` 1303, `pa-graph` 566 — all green.
- `cargo check --workspace --all-targets` clean; gate `--fast` (fmt + clippy) GREEN.

## fork.013 — upstream v0.10.3-beta.10

- Tag: `allspark-v0.10.3-beta.10-fork.013` (2026-11-03)
- Base: the `v0.10.3-beta.10` upstream merge (12 conflicts resolved), 22 upstream
  commits since the `v0.10.3-beta.1` merge (`381e2fadf`).
- No FORK-ONLY crate was touched: the upstream delta has zero files under
  `crates/pa-{graph,edit,lsp,ast,mem,jev,diff}` and zero fork docs/scripts.

### What the fork gained

- **`ipython` edit diffs reach the tool card.** The `python_repl` edit path renders
  its `diffs` in `details`; the fork's `toolArbiter` note was added beside it rather
  than instead of it.
- **A Google Workspace MCP server can log in with a user OAuth client.** A new
  `McpOAuthConfig.client_secret` is sent as `client_secret_post` on both the code
  exchange and the refresh, and a Google issuer gets `access_type=offline` +
  `prompt=consent` so a refresh token is actually issued. The fork's split
  `mcp/oauth/{login,refresh,ui}.rs` carry what upstream kept in one file.
- **`rlm.spawn` takes a `cwd`**, and a factory child no longer wakes its parent.
  `factory_child` threads through the fork's split `print_runtime/` modules to the
  engine config.
- **`agent_observe.recent_messages` reads a member no worker holds**, using the
  new `session_file` row field (the fork's inline print host passes `None`, since
  an inline node resolves its transcript from the tree).

### What the fork deliberately did not take

- **`bash` no longer has `yield_after`.** Upstream replaced the parameter with a
  fixed 1-second yield. The fork follows: the prompt line, the runtime-ready probe,
  and the `probe.rs` assertion all state the 1-second contract. `yield_after` was a
  fork-era knob and is gone from every surface.
- **The pickup checkpoint stays, against upstream.** Upstream deleted the RT-006
  write-ahead record from `worker/turn.rs` and left only the admission's snapshot.
  That reopens the defect the record existed for: the admission parks the row, the
  runner drains the lane in memory, the worker is killed mid-turn, and the relaunch
  restores the snapshot and executes the same prompt a second time — a duplicate
  side effect and a duplicate user row. The fork keeps `QueueCheckpoint::Delivered`
  and its append before the engine sees the rows.

  Proven red-first through the REAL runner (`TurnRunner::run`, slow scripted turn,
  abort mid-turn): without the append the journal still holds
  `run the side effect` after the pickup, and the test fails. With it, the journal
  is empty. Upstream's own tests do not cover this path — the fork's
  `a_picked_up_prompt_is_not_what_a_relaunch_replays` is the only one.
- **`parity-diff evidence` stays advisory.** Upstream's conflict side promotes
  parity to a merge gate; the fork keeps its 2026-10-19 decision (advisory, with
  wire compatibility binding). The new `.changes/<slug>.md` requirement was taken.

### Verification

- `pa-core` 1712 lib tests, `pa-daemon` 1276, `pa-graph` 558 — all green.
- `cargo check --workspace --all-targets` clean; gate `--fast` (fmt + clippy) GREEN.
- Every upstream behaviour fix has a test at its cheapest tier; the fork's own
  fixes (`StreamOutcome::Failed`, `lease_held_by`, `PI_OFFLINE`, `redact_tokens`,
  `push_user_turn`, `call_sites_for_callee`, `explain_change`, `AdrCodeRef`) are
  present and unchanged.

## fork.012 — upstream v0.10.2-beta.10

- Tag: `allspark-v0.10.2-beta.10-fork.012` (2026-10-27)
- Base: the `v0.10.2-beta.10` upstream merge (15 conflicts resolved), 58 commits
  since fork.011.

### Closed from the external audit (8 items, each with a red-first proof)

- **Discovery survives an offline restart.** The on-disk snapshot was read only
  inside a forced refresh, so a fresh process with no network lost every
  discovered-only id.
- **The packaging gates run from a clean tree.** Five release scripts were red on
  `main`: the launcher slices in `test_npm_bridge` read a `fork_layout` computed
  1200 lines above them, `test_native_compat` built a Namespace without the
  packer's field, and the restamp/verify fixtures were refused by the provenance
  gate. The waiver is now the CONSUMER's, pinned by a Rust test that no
  publishing workflow passes it — and by a second one that the four scripts
  nothing ran stay in CI.
- **`graph.auto = "watch"` watches.** The setting always accepted the value and
  documented itself as "behaves exactly like auto". It now starts a filesystem
  watcher over the session's cwd: bursts coalesced, `NativeWatcher`, a ZERO-wait
  writer lease (a busy lease drops the batch rather than queueing the session).
- **A prediction is not an observation.** A refinement event carried one
  `outcome` field, filled from the proposal's `expectedOutcome` — a guess written
  before anything ran, which the ranking prompt then showed the model as
  evidence. Split into `expected_outcome` / `outcome` in both layers, with a
  cross-language test that writes the Python payload and reads it back through
  the Rust loader.
- **One projection from the branch to the live loop.** Four paths replaced the
  agent's message list independently, and three rebuilt the context WITHOUT the
  late `agent_message.send` receipts, which live only in the derived view. After
  a compaction the tool result said "sent 0 messages" while the branch still
  carried the receipt.
- **A finished shell keeps its record until someone reads it.** Persistent bash
  deleted its rc/out files the instant the watcher saw the leader exit — before
  anyone read the result — so a shell that finished between turns lost its
  output across a restart. Deletion is now the ACK's business, with an explicit
  `discard()` for a caller that stops caring.
- **A second checkout with the same basename is indexable.** The project key was
  the directory basename and `register_project_at` refuses a name for a different
  root, so `/b/service` was permanently unindexable in a shared `graph.db`. The
  key is now the basename qualified by a short digest of the canonical root —
  only when a real collision exists.
- **`lsp.idleTimeoutMs` evicts an idle server.** The field was declared and
  documented as an idle bound while nothing read it, and the governor hid that:
  its "any identifier containing the field" rule credited the unrelated
  `resolve_stdin_idle_timeout_ms`. The rule is shape-based now, the setting has a
  reader, and an evicted client is not reported as a startup failure.

### Verification lanes that report

- **Miri** runs what it can finish, on the crates that have `unsafe`: the old
  lane had never passed (`pa-types` aborts on `posix_spawnattr_init`) and two of
  its three crates hold zero `unsafe`. A Rust pin fails if a named crate has none.
- **Kani** has harnesses for the first time: 5 proofs, 109 checks. The
  descriptor-limit decision was extracted from its syscall wrapper so a model
  checker can quantify over it. `kani::cover!` does NOT fail a run (measured), so
  vacuity is checked with assertions on concrete inputs.
- **Verus** grows a variant-set drift check (a mirror proving two of three enum
  variants verified cleanly), and `pa_core_truncate.rs` is the first mirror with
  NO trust surface — it passes `--no-cheating`.
- **Mutation testing the proofs** (`cargo-verus-mutants`) found three real spec
  holes in the decision kernel, all of the shape "a branch whose outcome nothing
  constrained". 4 mirrors / 28 obligations / 95% kill on the full 34-mutant set.
- **Shuttle** now covers the cron claim path: two dispatch passes must never
  claim one job twice, with a companion test proving the same invariant without
  concurrency — a mutex says nothing about the function under it.
- **faux provider** can assert its whole script was consumed
  (`expect_consumed`), so a test that stops meaning what it says fails.

### Found while looking for the next proof subject

- **No signalling path can reach `kill(-1)`.** `kill_process_group_or_pid`
  floored at `> 0` while its three siblings floored at `> 1`, so `pid == 1`
  became `kill(-1, SIGKILL)` — every process the caller may signal. One predicate
  now gates all four paths, proved over every `i32`.
- **`lsp::ensure_client` no longer serializes every project.** It held a
  roster-wide write guard across a spawn plus an LSP handshake; the single-flight
  is per key now.

## fork.011 — upstream v0.10.2-beta.3 (+v0.10.2-beta.7 merge)

- Tag: `allspark-v0.10.2-beta.3-fork.011` (2026-10-26)
- Base: `5a3df6fc9` line continues from fork.010; 51 commits, of which six are
  an upstream `v0.10.2-beta.7` merge (tag-only snapshot: compaction duplicate
  rows, queued-work hold during navigate/fork, session-event isError, MCP
  oauth trailing-slash issuer, google systemInstruction shape, instant
  @-completion, tokio 1.54.1 for RUSTSEC-2026-0386).

**Identity and attach (the empty-picker incident class).**
- `85c46975a` — a content build stamp: `build.rs` resolves HEAD (12-hex,
  `.dirty` when the tree is unclean, rerun-if-changed on the git refs), the
  stamp rides `daemon_hello` as `runtime.build_hash`, and `--version` carries
  it (`; build <hash>`). A version string is not an identity.
- `3449ba879` — probe and doctor classify daemon compatibility through ONE
  predicate (`build_compatible`, fail-closed: no stamp = `Unknown`), replacing
  three classifiers that disagreed on different field sets.
- `88cfed495` — `--version` names the content hash, so two unstamped local
  builds are distinguishable at a glance.

**Live fixes the user hit.**
- `cbe6f1610` — muse-spark-1.1/1.2 reject `reasoning_effort=max`; the catalog
  now advertises only what the API accepts (three models were failing).
- `d5e4fb430` — LSP line-addressing resolves to the identifier ON the line
  (innermost wins), not to column 0.
- `430aece2c` — `.graphignore`: the repository's own ignore file for the code
  graph.
- `317cfc4a1` + `99fb1345a` — the release pipeline ships real catalog assets
  (the packer gate fires for real; the disk guard reads a number), `--network`
  replaces `--fixture` as the release path.
- `fb8074008` — the packaging dry-run packs its fixture catalog with an
  explicit waiver (the gate kept firing on offline dry-runs).
- `8e27d317a` — `update --rollback` sets its payload root (a regression from
  the publish rework; both installer e2e rollback tests went red without it).

**Verification infrastructure (issue #14 ladder + ADR-0001).**
- `86b89e702` + `59c27ab1d` + `1ec0b6ce3` + `e9a788aeb` — the test-contract
  governor (FSM interpreter over dictionaries, 33-row matrix, selftest proves
  every check can fail), the dynamic model discovery engine (muse, subq,
  opencode, agnes), Miri/ASan/Shuttle workflows, the design-decision log
  (D1–D9), and the perf-review procedure with baselines.
- `9f12c7eb2`..`3eeb2ce7f` — one shared PTY harness replaces 11 drifted
  per-test copies; the quiescence oracle (`wait_for_stable`), failure
  artifacts (raw.log + screen.txt), and the set-equality oracle end to end
  against the real binary.
- `5b41318be` — Agnes AI as a provider (4 models); `86b697b67` — path-routed
  mock responses so one test can hold two wire dialects apart.

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

- `9a582825e` — a context's `fixed_tools` reaches the tool pipeline. `Context`
  carried it and `Context::apply` honoured it, but `Context::delta()` dropped it, so
  `minimal` ("bash + edit only") handed the runtime the full roster. `query` also
  said "Read-only surface" while excluding just `edit` and `bash`; `python_repl` can
  write files, so the description now says what the layer does.
- `a23e049dc` — a refused child delete no longer claims the child is cancelled.
  Both cancellation paths set `cancelled` before `await delete_subagent`, so a
  refused delete left a live child under a `stopped` run. The claim is now
  `cancelling`, the terminal status follows the delete's result, and `stop()`
  reports the survivors it could not cancel.
- `a49fd1bd3`, `4fb017bab` — a judgment is priced and capped per ENDPOINT, not per
  chain. One constant priced every endpoint at TypeSafe's 0.042 while SubQ Snap bills
  0.05, and `max_questions` was never set, so a 17-question `ask` hit SubQ's 422
  instead of chunking.
- `8e2af5056` — a re-ranked hit reports `kept`, not `verified`. Clearing a 0.1
  threshold means "not confidently rejected", not "confirmed".
- `140f02a35` — `allspark update` on a fork build refuses instead of installing
  upstream over itself. Every endpoint the funnel reaches belongs to upstream; the
  fork marker had only ever reached the installer's daemon-stop probe.
- `76efd5959` — `FactoryExecutor`'s docstring says what a kernel restart costs: the
  run id is unknown afterwards, `status`/`stop`/`resume` raise, and surviving
  children must be stopped through `rlm.delete_subagent`.
- `f07be9ec1` — the F05/F06/F17 findings: an LSP timeout that bounded only the final
  request (8.15 s -> 0.00 s on a zero budget) now bounds the whole cycle; `graph.auto =
  "watch"` is documented as accepted-but-equal-to-`auto` (`fs_watcher` was never wired);
  an import resolved by suffix guess no longer wears the exact-match 0.95 — it is demoted
  to 0.55 and the ambiguity is reported.
- `8c107093b` — a judgment answer must be about the question that was asked. The
  decoder compared the answer's kind and stopped, so a choice naming an option the
  question never offered, a score past the rubric, or a probability of 1.5 all came back
  as judgments. The last open audit finding; the one that changes behaviour.
- `64bc6e007` — the toolchain pin moves to **1.99.1** in both halves (`rust-toolchain.toml`
  and the dtolnay action SHA in seven workflows), after two real 1.99 findings:
  `clippy::assert_is_empty` at `activity_lines.rs:53` and `late_sent_messages.rs:405`.
- `733daae68`, `c5409e17e` — a reindex must move the evidence AND resolve like the full
  index: the receipt chain stayed behind a reindex (`verify` accepted a store that no
  longer existed), and the incremental resolver was dependency-blind, so a reindex
  deleted cross-crate CALLS edges the full index wrote (487 in the live store).
- `aef9d78df` — local harness state is written where the kernel and digest read it:
  the writer used the shared sessions parent while everything else used the
  per-session artifact dir, so 55 refinements sat in a file nothing ever opened.
- `0c24251d5` — service notices must not spend the message budget: 4 000 `[bash-done]`
  rows filled the 20-row queue and a subagent report was refused with "184 unfinished,
  limit is 20". Notices now replace by key (`bash:<pid>`), carry their own ceiling, and
  the delivery guard counts only real messages.
- `20a4e3d99` — OpenCode retirements are recorded, dated, and kept resolvable instead
  of being silently offered after the endpoint stopped serving them (the discovery
  contract, `docs/model-discovery-contract.md`).
- `effab4122` — stall diagnostics name a clean stop that announced an action it did not
  take (`stall_diagnostic`, transcript-only; muse-spark announced a next step and
  stopped in 22 of its 44 stops).
- `84016ab29` — muse contributor models stop replaying prior reasoning items: the
  upstream Console backend rejects their `encrypted_content` across rotated accounts,
  which surfaced as a turn that printed its next action and died (9router `eafac37d`,
  ported as mechanism).
- `ce6d85fd0` — a streamed tool-call fragment without an id gets a synthesized stable
  one, so strict OpenAI clients stop aborting the turn with "Expected 'id' to be a
  string" (9router #4992, ported as mechanism).
- `5fbe3e6d3` — the `subagent-keep-alive` valve gets its consumer: descendant work
  unsettled past the configured window admits the keep-alive text while the autonomous
  continuation stays held, instead of the deferral staying silent forever.

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
