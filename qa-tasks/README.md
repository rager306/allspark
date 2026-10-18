# Real-Task Deviation Suite (levels 1+2)

Black-box behavioral QA for the allspark fork: short tasks distilled from
REAL failures of this fork (`.research/verify-*.md`, `re-verify-report.md`,
the execution plan), each run end-to-end against the real `prime-agent`
binary in a throwaway sandbox and graded by a STATE oracle plus
trace-level invariants - never by matching the answer text.

Strategy source: `.research/blackbox-qa-best-practices.md` §3
(level 0 = capture/manifests, level 1 = deterministic task oracles,
level 2 = trace asserts + behavioral fingerprint; levels 3-4 judge tiers
are future work).

## Layout

```
qa-tasks/
  RT-<id>/task.yaml        the task (grammar below)
  RT-<id>/setup.sh         builds the isolated trial environment
  RT-<id>/faux-script.json scripted-model lane (deterministic CI runs)
  RT-<id>/oracle.py        script oracle (only for tasks that need one)
  RT-<id>/reference/       reference solution + solvability argument
scripts/qa/run_suite.py    the runner (stdlib only)
out/<ts>/manifest.json     run manifests (generated; not part of the corpus)
```

## task.yaml grammar (strict mini-YAML)

The runner parses a strict YAML subset (no anchors/flow/multi-doc); every
task.yaml must also parse with real YAML. Supported: 2-space nesting,
`key: scalar`, quoted scalars, `key: |`/`|-` block scalars, `- item`
lists (scalars or nested maps), `#` comments.

```yaml
id: RT-XXX-name
title: one line
origin: "finding #N (doc), fix <sha>"     # provenance into .research/
difficulty: easy | medium | hard
lane:                # which lanes can run it (first runnable is default)
  - faux             # scripted model via PRIME_AGENT_FAUX_SCRIPT
  - mock-provider    # loopback wire mocks via models.json (no network)
  - real-model       # live provider via --provider/--model/--api-key
  - daemon           # ACP/daemon driver (see Missing seams)
network: off | loopback
budget: {tokens, steps, timeout_s}         # all enforced by the runner
entry: print | launcher                    # launcher = allspark layout
bin_args: [...]                            # extra argv before -p
setup: setup.sh                            # cwd = trial work dir
prompt: |-                                 # the agent's task
  ...
files: [big.log, img1.png]                 # @file args after the prompt
mock_providers:                            # runner-hosted loopback mocks
  - {name, behavior, answer}
hostile_env: {VAR: canary-path-or-rel}     # sandbox-joined hostile ambient
secrets_env: {VAR: value}                  # planted + grepped + redacted
oracle:
  type: state | script
  spec: ...                                # state checks (below) or oracle.py
must:                                       # level-2 required behaviors
  - {tool: bash, args_regex: 'git stash'}   # >=1 matching tool call
  - {row_type: compaction, min: 1}          # >=N transcript rows of type
  - {transcript_contains: s} / {outcome_contains: s}
must_not:                                   # forbidden behaviors
  - {tool: bash, args_regex: 'rm\s+-rf\s+/'}  # 0 matching calls
  - {transcript_contains: s} / {outcome_contains: s}
max_repeat_calls: 4                         # identical-call loop bound
kill_worker: {marker, delay_s, needle}      # daemon lane only
reference: reference/solution.md
```

State-oracle checks: `{path, exists|not_exists|contains|not_contains|size_lt|occurrences_ge|occurrences_le}`
(the occurrence pair takes `{text, count}`: the exactly-once primitive),
`{transcript_contains|transcript_not_contains|transcript_occurrences_le}`, `{exit_code: N|nonzero}`,
`{duration_le: s}`, `{mock_requests_le|ge: {name, count}}`,
`{dir_empty: abs}`, `{dir_no_new_jsonl: {dir, since}}`.
Script oracles read `QA_HOME/QA_WORK/QA_TRANSCRIPT/QA_SANDBOX/QA_MOCK_*`
and exit nonzero with reasons on failure.

## The seed corpus (12 tasks, all from real fork failures)

| id | origin | lane | invariant (state, not text) |
|----|--------|------|------------------------------|
| RT-001-provider-402-bounded | verify-reliability #7 / 2e79330c3 | mock-provider | bounded requests, error surfaced, no death-loop |
| RT-002-stalled-provider-timeout | verify-reliability #8 / 2e79330c3 | mock-provider | stalled peer settles via request timeout |
| RT-003-atfile-embed-cap | verify-unbounded #4 / 5461ba80a | faux | 256KiB prefix + truncation marker, tail absent |
| RT-004-kernel-restore-write-handle | #23 / abe2a419b | faux | open write handle survives kernel death (file not truncated) |
| RT-005-kernel-error-clip | verify-unbounded #11 / 58394f09a | faux | 1.5MB error clipped at 65536 + marker |
| RT-006-worker-crash-exactly-once | P0 #3 / 2b3d74842 | daemon | side effect exactly once after worker SIGKILL (**RED today**: the relaunched worker re-delivers the prompt) |
| RT-007-launcher-env-isolation | fork-identity / 5122da3d3 | faux (launcher entry) | hostile PRIME_AGENT_* stays inside the sandbox |
| RT-008-secret-env-not-in-transcript | gsd-pi name-only | real-model + faux | planted secret values never in transcript/outcome |
| RT-009-dirty-tree-destructive-git | #17 class | real-model | WIP recoverable after hard sync to origin/main |
| RT-010-compaction-image-heavy | verify-unbounded #13 / 07364009a | faux | compaction row before overflow on image-heavy history |
| RT-011-tz-stable-git-timestamps | 37e788625 (TZ determinism) | real-model | commits.txt byte-identical under TZ=UTC/+07 |
| RT-012-failover-402-backup | #7/#38 + provider_failover_e2e | daemon | backup provider answers after primary 402 |

Known-red-on-purpose: RT-007 (the launcher does not pin
`PRIME_AGENT_SESSION_DIR`, so an ambient value escapes the isolated home)
is expected RED today; it documents a real isolation gap and the baseline
manifest records it. RT-004 is red-first proven at the runtime level
(pre-fix runtime truncates to 0, fixed runtime keeps 1700 bytes; see its
reference/) and green end-to-end once the kernel venv carries the fixed
runtime. A task being red is DATA; the delta gate decides regressions.

## Running

```bash
# deterministic lanes (faux + mock-provider), 3 trials each:
python3 scripts/qa/run_suite.py --trials 3

# filter + single trial while iterating:
python3 scripts/qa/run_suite.py --tasks RT-003 --trials 1 --keep

# real-model lane (needs creds):
python3 scripts/qa/run_suite.py --lane real-model \
    --provider <provider> --model <model> --api-key <key>

# daemon lane (spawns the supervisor + workers inside each trial sandbox):
python3 scripts/qa/run_suite.py --lane daemon --tasks RT-006 RT-012

# regression gate vs a baseline manifest:
python3 scripts/qa/run_suite.py --baseline out/<ts>/manifest.json
```

Binary resolution: `--bin` > `PRIME_AGENT_BIN` > `target/debug/pa-cli` >
`target/debug/prime-agent` > `target/dist/prime-agent` > `$(which
prime-agent)`. Kernel interpreter for ipython-lane tasks: `--kernel-python`
/ `QA_KERNEL_PYTHON` (default: `~/.prime/agent/kernel-venv/bin/python`;
see RT-004's deployment-gap note).

Manifest: `suiteRev` (content hash + git head), `binaryBuild`
(`--version`), per-trial results (exit, duration, steps, tokens, tool
frequencies, failure reasons), and the level-2 fingerprint
{tool share, median tool turns, p95 duration, cancel share, subagent
share}. Fingerprint deltas are advisory; the gate is: trial pass-rate
drop >= 3pp, or >= 2 stable (pass-all in baseline) tasks losing pass-all
=> exit 1. One stable task newly failing = yellow (future judge tier).

## Isolation contract (every trial)

* Fresh tmp sandbox: `home/` + `work/`; `HOME`, `PRIME_AGENT_CODING_AGENT_DIR`,
  `XDG_DATA_HOME` pinned inside it; ambient `PRIME_AGENT_*`/`PI_*`/`PA_*`,
  `RLM_*`, `GH_TOKEN`, `GOOGLE_*`, provider keys scrubbed.
* `DO_NOT_TRACK=1`, `--offline` on network=off tasks; loopback-only mocks
  on network=loopback tasks (the harmful-pattern checker flags non-loopback
  network on off-tasks).
* A random `QA_SUITE_CANARY` secret is planted in every trial's env and
  grepped in transcript + stdout/stderr: the no-secret-leak invariant is
  exercised suite-wide, not just on RT-008.
* Full transcript copied per trial to `out/<ts>/<task>/trial-N/`.

## Adding a task (PR process)

1. Root the task in a REAL failure (issue/finding/commit); cite it in
   `origin`. No invented scenarios.
2. Oracle on STATE (files, git plumbing, transcript row types, wire
   counts) - never on answer wording. If a check needs prose, it belongs
   to the judge tier, not here.
3. `reference/` must contain a working solution and a solvability
   argument ("why this is achievable by the system under test").
4. Two-experts rule: a task merges when two reviewers independently
   agree that (a) the oracle cannot be satisfied by a broken agent and
   (b) a correct agent cannot fail it spuriously (no TZ/locale/timing
   dependence outside what the task tests).
5. Balance must/must_not (required call AND forbidden call) where the
   behavior allows; budgets tight enough to catch loops, loose enough
   for honest work.
6. Faux lane where the invariant is harness-owned; real-model lane where
   it is judgment-owned; mock-provider lane for wire behavior.
7. Red-first: a task that cannot fail is dead weight - demonstrate (or
   argue from the fix's red test) that the pre-fix system fails it.

## Anti-patterns (rejected by design)

* Exact-sequence asserts on tool calls (fragile, punishes valid paths).
* Oracles on answer text / string-matching prose.
* Fixed sleeps for readiness (mocks bind before setup; the binary run is
  one `communicate(timeout)`; the single poll loop is the daemon-kill
  marker wait, deadline-bounded).
* Tasks that need the network (everything is faux, loopback, or
  explicitly real-model lane).
* Unversioned comparisons: a run without its manifest is not evidence;
  changing the corpus or system prompt = new `suiteRev`/release.
* Growing this tier into judging: LLM-graded calls live in the future
  level-3 tier, never in the CI gate.

## Lane findings baked into the corpus (2026-10-17 smoke)

The daemonless `-p` print path runs WITHOUT the daemon-side supervisors.
Verified live against `target/dist/prime-agent`:

* **no failover re-route**: a 402 primary fails the turn; the backup
  provider sees 0 requests (the re-route driver is
  `pa-daemon/src/agent_engine/turn/run_once.rs`; "Faux-script sessions
  never fail over"). RT-012 is therefore daemon-lane.
* **no proactive compaction**: a scripted session sailed to 175% of a
  20k window with `compaction.enabled` set and produced 0 `compaction`
  rows (threshold supervision is `pa-daemon/compaction_supervision.rs`).
  RT-010 is therefore real-model/daemon-lane.
* **stall bound is hardcoded 300s** on the print path
  (`PROVIDER_REQUEST_TIMEOUT_MS` in `provider_adapter.rs`); the
  `retry.provider.timeoutMs` setting is read only by the daemon runner.
  RT-002 budgets for the 300s bound (~5.5 min/trial - run 1 trial in the
  smoke tier).

These are suite results in their own right: headless `-p` consumers get
neither failover nor proactive compaction.

## The daemon lane (`--lane daemon`)

RT-006 and RT-012 need the DAEMON turn runner (failover re-route and the
worker supervisor live there; the daemonless `-p` path has neither). The
runner drives it directly over the supervisor's JSONL socket - the same
wire the ACP mode and the e2e harnesses speak:

1. **spawn**: `prime-agent --mode daemon --daemon-socket <sandbox>/daemon.sock`
   under the trial's pinned env (scrubbed `PRIME_AGENT_*`, sandbox `HOME`),
   stdout/stderr into `trial-N/daemon.log`. Readiness = the socket file
   appears (never a timer).
2. **handshake**: connect, read `daemon_hello` (protocol name + version are
   taken FROM the hello, so a protocol bump cannot silently desync the
   driver).
3. **create + attach**: `{type: create, config: {cwd, sessionDir,
   provider?, model?, apiKey?, script?}}` then `{type: attach,
   activeSessionId}`. A task's `faux-script.json` rides the create's
   `script` key (the daemon-side equivalent of `PRIME_AGENT_FAUX_SCRIPT`);
   the model selection comes from `--provider/--model` or the task's
   `bin_args`.
4. **turn**: `{type: prompt}` then `{type: wait_for_idle}` bounded by the
   trial budget; at the deadline the driver `abort`s and grades what the
   session produced (the print lane's `communicate(timeout)` kill is the
   same policy). The turn's text is read back via
   `get_last_assistant_text` + the streamed `session_event` frames and
   becomes the trial's outcome text, so `outcome_contains` oracles grade
   both lanes identically.
5. **kill hook** (`kill_worker: {marker, delay_s}`): once the scripted
   turn's marker file lands, SIGKILL the worker pid from the durable
   descriptor (`<agent-dir>/daemon-workers/<key>/<workerId>.json`) - the
   product's own identity record, not a /proc guess - then wait for the
   descriptor to publish a NEW pid (the supervisor's relaunch).
6. **transcript**: the session JSONL under `<agent-dir>/sessions/` is
   copied to `trial-N/session.jsonl` and graded by the same level-2 layer
   as every other lane.
7. **stop**: `{type: shutdown}` (the supervisor's graceful path), then the
   process group is reaped; a wedged supervisor gets SIGKILL.

A daemon session exposes the kernel-backed `python_repl` tool (the model
tool roster is `tools: vec![]` + the engine's own kernel tool), so a
daemon-lane `faux-script.json` must script `python_repl`, not `bash`.
Daemon sessions also carry the full worker system prompt (~10.8k input
tokens per model call), so daemon-lane token budgets are ~3x the print
lane's for the same work - RT-006's budget comment records the measured
numbers.

## Missing seams (honest holes; see run report)

* **daemon lane + task files** (RT-010): the print lane turns `files:`
  into `@path` argv; the daemon's create/prompt carries no argv and the
  driver attaches no images yet, so a daemon run of a `files:` task would
  grade a prompt the task never asked for. Those tasks SKIP loudly on
  this lane instead of reporting a misleading red.
* **shared kernel venv** (RT-004/005): the default kernel interpreter is
  the machine venv; the product upgrades its embedded runtime in place on
  version mismatch. CI sandboxes should pin `QA_KERNEL_PYTHON` (or accept
  first-trial upgrades) - the manifest records the interpreter.
* **launcher SESSION_DIR pin** (RT-007): red until the launcher pins
  `PRIME_AGENT_SESSION_DIR` like it pins the agent dir and socket.
* **real-model creds in CI**: the real-model lane exists but has no
  credentialed CI lane yet (faux + mock-provider are the CI gate).
