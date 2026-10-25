# Grok-build test infrastructure vs ours — item-by-item, with the red-first test for each gap

Date: 2026-10-24. Read-only survey of `/root/prime-agent` (allspark fork). Nothing was built, run,
edited or committed; the only write is this file. Every claim about us carries `file:line`, verified
with `rg` in this tree. Claims about Grok are the task brief's summary and are marked `grok:brief`;
I did not read the Grok tree, so anything of their shape is quoted as given.

## 0. Ground truth measured in this tree (used by every cost estimate below)

| fact | number | how measured |
|---|---|---|
| integration test targets | 239 (235 top-level `*.rs` + 4 directory targets) | `crates/*/tests/*.rs` + `*/tests/*/main.rs` |
| `#[test]` / `#[tokio::test]` functions | 3578 | `rg -c '^#\[(tokio::)?test\]' crates/` |
| test files that hand-roll a loopback HTTP server | 27, 15,336 lines | `rg -l 'TcpListener::bind' -g '**/tests/**' crates/` |
| files that open a pty | 17, 11,374 lines | `rg -l 'openpty\|posix_openpt' crates/` (minus `Cargo.toml`) |
| distinct `static HARNESS_LOCK` declarations | 11 | `rg -n 'static HARNESS_LOCK' crates/` |
| literal `Duration::from_secs(N)` in `pa-cli/tests` | 191 | `rg -o 'Duration::from_secs\(\d+\)' crates/pa-cli/tests` |
| sandbox/home builders (`fn sandbox`, `struct Sandbox`, `fn isolated_home`) | 14 definitions in 12 files | `rg -n 'fn sandbox\(' crates/` plus the two sibling patterns |
| `#[ignore]` attributes | 80 | `rg -n '#\[ignore' crates/` |
| insta snapshots | 0 | `find crates -name '*.snap'` |

---

## 1. The twelve items

Legend for verdict: **have** = a real equivalent exists and is wired to the place that would catch
the defect; **partial** = the primitive exists but is not reachable from where the defect lives;
**absent** = nothing.

| # | Grok item (`grok:brief`) | Ours (`file:line`) | Verdict | Effect if closed | Cost | Priority |
|---|---|---|---|---|---|---|
| 1 | `xai-grok-test-support`: one shared crate (mock inference, mock OTLP, mock managed-config, SSE gen, ACP client, headless runner, TestSandbox/TestProcess) | No such crate. The mock inference server is a **feature-gated module inside the production crate**: `crates/pa-ai/src/testing/mod.rs:1-15`, `crates/pa-ai/src/testing/mock_server.rs:103`, gate `crates/pa-ai/src/lib.rs:23-24`, feature `crates/pa-ai/Cargo.toml:8-12`. Every other double is re-derived per test file (§1.8, §1.11) | **partial** | One home for doubles; the 27 re-rolled HTTP servers and 17 pty harnesses stop drifting | 3-5 d for a `pa-testkit` crate; ~1 d per 5 files migrated | medium (real but large; do it incrementally, see §5 item 2) |
| 2 | `MockInferenceServer`: mock `/v1/chat/completions`, `/v1/responses`, `/v1/messages`, scripted replies | **Yes, and it is stronger than the brief assumes.** `MockInferenceServer::new` at `crates/pa-ai/src/testing/mock_server.rs:125`; scripting `queue_response:195`, `queue_json:204`, `queue_sse:210`, `queue_sse_dropping:222`, `queue_sse_truncated:234`, `queue_sse_raw:245`, `queue_hang:258`, `queue_sse_stalling:265`; observability `queued:280`, `requests:290`, `request_count:300`; request capture `RecordedRequest:27`, `header:39`, `body_json:48`; drop releases held connections `:315-328`; queue exhaustion answers a loud 500 `:373-383`. Real HTTP/1.1 + chunked SSE, not an in-process shim. Consumed today by 3 places: `crates/pa-daemon/tests/provider_failover_drill_e2e.rs:229-572` (9 servers across its drills), `crates/pa-core/tests/provider_session_id_wire.rs:147,174`, `crates/pa-ai/src/utils_inner/http.rs:542,599`. Two more files re-implement the same pattern instead of using it — `crates/pa-jev/tests/client.rs:31` `struct MockServer` (its header even says "the pa-ai `testing::mock_server` pattern, trimmed", `:1-6`) and `crates/pa-core/src/jev/tests.rs:270` | **have** | — | — | — |
| 2b | *…but* routed by path (`/v1/messages` vs `/v1/chat/completions` vs `/v1/responses`)? | **No path routing.** `serve` pops the next queued response regardless of the request target (`mock_server.rs:338-372`); only `completions_url:190` exists. Consequence: two providers on one server, or an Anthropic-shaped body under `/v1/messages`, cannot be distinguished by path — the *test* must know the arrival order | **partial** | Lets one server serve a mixed OpenAI/Anthropic failover pair and lets a test assert *which* dialect the agent chose | ~40 lines: match on `request.path`, fall back to the FIFO | high (cheap) |
| 3 | `conversation_script` DSL: `Conversation::nth(n).calls([...]).reply(...)` | Two scripted-turn seams exist, neither a DSL: (a) the whole-process **faux provider** `PRIME_AGENT_FAUX_SCRIPT` — parse `crates/pa-cli/src/print_runtime/faux.rs:18`, per-child lane `:29-33`, provider registration `:82-113`; script grammar `crates/pa-ai/src/providers/faux/script.rs:39-157` (text, `content[]` blocks of thinking/text/toolCall, `stopReason`, `errorMessage`, `delayMs`, `tokensPerSecond`, `repeatLastResponse`); (b) the loopback server's FIFO queue (`mock_server.rs:195-278`). Neither has a *turn-numbered expectation*: a script can be silently under-consumed | **partial** | See §1.7 below — the missing half is the *consumption* assert, not the scripting | 1 d | high |
| 4 | `sandbox/TestSandbox`: isolated HOME, tempdirs, env zeroing | **Four different `fn sandbox(`** in `pa-cli/tests` (a fifth lives in `pa-core`'s installer unit test), 7 `struct Sandbox`, 2 `fn isolated_home` — 14 definitions in 12 files: `crates/pa-cli/tests/differential_cli.rs:454` (`fn sandbox(prefix: &str) -> PathBuf`, builds `home/cwd/agent/tmp`, sets `TMPDIR`, `PI_OFFLINE` at `:331-344`), `crates/pa-cli/tests/lock_compat_e2e.rs:42` (same signature, different env set at `:59-62`), `crates/pa-cli/tests/package_resources_e2e.rs:45` (`fn sandbox(settings) -> Sandbox`, pre-writes `settings.json`, `env_remove` list at `:66-68`), `crates/pa-cli/tests/packaged_layout_e2e.rs:175` (`fn sandbox() -> Sandbox`, `env_remove` list at `:195-202` incl. `PI_PACKAGE_DIR`, `PRIME_AGENT_KERNEL_*`, `PRIME_API_KEY`), plus `print_runtime_e2e.rs:119`, `package_e2e.rs:234`, `installer_update_e2e.rs:89`, `blackbox_coding_task.rs:40`, `multi_client_e2e.rs:501`, `pa-core/src/packages/tests.rs:106`, `pa-core/src/update/installer.rs:1320` | **partial** | Each copy is a *different* hermetic contract, so a variable one test scrubs another leaks — `RT-007-launcher-env-isolation` documents exactly this class (a launcher that does not pin `PRIME_AGENT_SESSION_DIR`) and is red on purpose (`qa-tasks/README.md:96-97`, `:265`) | 1-2 d: one `TestSandbox` in a test-support module, migrate the 14 definitions | **highest** (cheap, closes a live defect class) |
| 5 | `headless` runner: `grok -p` against the mock server with a timeout | A Python runner does exactly this: `scripts/qa/run_suite.py:1442-1480` (`-p` lane, one `communicate(timeout=task.timeout_s)`, `:1472`), budget from `task.yaml` (`:447-448`). In Rust there is **no** helper: tests either shell out to `timeout` (`differential_cli.rs:327`, `package_e2e.rs:289`, `lock_compat_e2e.rs:54`) or hand-roll deadline loops, with 191 literal `Duration::from_secs` values in `pa-cli/tests` | **partial** | A loaded runner turns into a red suite with no single knob; the gate already had to fence 5 units into a serial tail (`scripts/fork/prepush-gate.sh:263-271`) | ~10 lines: `scaled(Duration)` reading `PRIME_AGENT_TEST_TIMEOUT_SCALE` + a `run_headless(bin, args, sandbox, timeout)` helper | high (cheapest item in the list) |
| 6 | `mock_otel_server`: mock OTLP collector, leak/limit checks | **Absent.** Zero `otlp`/`OTEL_EXPORTER` hits under `crates/pa-telemetry/`; no `opentelemetry*` dependency anywhere in `Cargo.toml`/`Cargo.lock`; the product ships **no OTLP exporter** — telemetry is the JSON `AnalyticsSink` (`crates/pa-telemetry/src/sinks/analytics.rs`) plus the local `FileSink` mirror (`sinks/file.rs:18-46`) | **absent** | The *reason* for their mock (a real OTLP exporter leaving the machine) does not exist on our side, so a mock collector would verify a protocol we do not speak. The transferable part is narrower: **the canary over the bytes we do send** — see item 9 | 0 for a collector; 0.5 d for the canary | not needed (see §4) |
| 7 | ACP stdio harness (`GrokStdioClient`): real pipe connections, protocol assertions | **Yes, twice.** `crates/pa-cli/tests/acp_mode_e2e.rs:27` (`struct AcpChild`), reader thread `:58`, `AcpChild::spawn` `:87`; 2701 lines asserting against a TS capture corpus (`crates/pa-daemon/testdata/acp/README.md`, stated at `crates/pa-cli/tests/acp_mode_e2e.rs:13-17`). A second, independent copy: `crates/pa-cli/tests/acp_config_option_e2e.rs:36` (460 lines, 1 test) | **have** (duplicated) | — | 0.5 d to fold the second copy onto the first | low |
| 8 | `run_agent_test` in `acp_harness/mod.rs`: start mock, isolate HOME, fake key, run the body against the real agent | The composition exists per test, not as a function: the ACP e2e starts a sandboxed supervisor + scripted faux worker (`acp_mode_e2e.rs:13-17`), `blackbox_coding_task.rs:1-22` runs a whole task through the real binary with a faux model, and `qa-tasks/` does it in Python across 4 lanes (`qa-tasks/README.md:36-43`). What is missing is the **single reusable entry point** (`run_agent_test(body)`) — today each file re-wires sandbox + script + binary | **partial** | A new behavioural test costs ~200 lines of setup instead of ~20, which is why the doubles drifted | 1.5 d (needs item 4 first) | medium |
| 9 | `xai-grok-tools`: unit per tool; `full_toolset_descriptions_render_cleanly` smoke over **all** tools | Per-tool unit tests exist and are good: `crates/pa-core/src/tools/graph.rs` 32 tests, `edit_hybrid.rs` 14, `truncate.rs` 5, `path_utils.rs` 6, `ipython.rs` 3, `bash.rs` 4; schema pins at `graph.rs:2369` (`the_schema_names_the_parameters_the_actions_read`), `edit_hybrid.rs:697`, `crates/pa-lsp/src/types.rs:355`. **But the TS-differential schema check covers only 3 tools** (`crates/pa-core/tests/golden/corpus/schema.json` `caseCount: 3` — bash/edit/ipython, asserted at `crates/pa-core/src/tools/golden_replay.rs:574-615`), and **no test iterates the live roster** (`lsp`, `graph`, `edit`, `jev` — constructed at `crates/pa-core/src/session_engine/engine.rs:632,705,741,755` and named at `:637,707,748,758`) asserting non-empty description / valid JSON-Schema shape / no unresolved template artifact. The prompt-side guard is adjacent but different: `crates/pa-core/tests/prompt_guards.rs:563` checks the *prompt documents the surface*, not that a tool's `description` renders | **partial** | A tool whose `description` is empty, or whose `parameters` is not an object schema, ships silently and the model calls it blind. This is the "documented, never wired" class the repo's own audit list names | 0.5 d: one test over the roster entries | **highest** (cheap, mechanical) |
| 10 | `prompt_parser.rs` / `attach_file.rs` unit tests on the XML they assemble (`<file_contents>`, `<focused_files>`) | Attachment side **have**: `crates/pa-cli/src/file_processor.rs:44` (`process_file_arguments`) with 12 tests at `:138-327` — `<file name>` blocks, image base64, 256 KiB cap + truncation marker (`:234-253`), multibyte-safe cut (`:292-315`), cap boundary (`:256`). Prompt side **have**: `crates/pa-core/tests/prompt_guards.rs:61,79` (static layers contain no dynamic content; cached prefix byte-stable across sessions). **Absent**: `<focused_files>` / `<file_contents>` do not exist in our wire at all (`rg -n 'file_contents' crates/` and `rg -n 'focused_files' crates/` → both 0 hits) — that is a Grok prompt shape, not a missing test | **have** (for our shapes) | — | — | — |
| 11 | `xai-grok-pager-pty-harness` (`PtyHarness`): real binary in a PTY, key injection, `#[ignore]` gated | **Yes, 17 times over** — pty spawning in 17 files, 11,374 lines, plus a proper reusable harness that is *not* reusable: `crates/pa-perf/src/bench/pty.rs:28` `pub(crate) struct PtyChild` (spawn `:49` openpty+setsid, `wait_from:117`, `wait_screen_contains:214`, `wait_screen:230`, `screen_text:257`, `pid:262`, `finish:267` SIGTERM→2 s→SIGKILL, `Drop:292`), vt100 screen model, kitty probe answered once, split-safe (`:195-205`). It is `pub(crate)` under a private `mod pty` (`crates/pa-perf/src/bench/mod.rs:135`) and **`pa-perf` is nobody's dependency** (`rg 'pa-perf' crates/*/Cargo.toml` → only its own manifest), so `pa-cli/tests` cannot use it. Working alternatives in-tree: `crates/pa-cli/tests/terminal_state_differential_e2e/harness.rs:41` `DifferentialHarness` (694 lines of harness, `pub(crate)`, same crate only), `crates/pa-cli/tests/login_panel_pty_e2e.rs:530` openpty, `crates/pa-cli/tests/kitty_exit_routes_e2e/main.rs` (787 lines, 2 pty tests) | **partial** | A pty assertion is copied, not shared: 11 files re-declare their own `HARNESS_LOCK`, and pty/termios calibration slips (the gate already serializes 5 of them) | 2 d: promote the harness to a `pa-testkit`/`pa-pty` module usable from `pa-cli` tests | high |
| 12 | `xai-grok-pager-pty-harness` consumer of `TestSandbox`/`TestProcessTree`/`MockInferenceServer` (one composition) | Closest composition: `crates/pa-cli/tests/terminal_state_differential_e2e/` (harness + ledger + main), `crates/pa-daemon/tests/multi_client_e2e.rs:501` (pty + sandbox), `crates/pa-cli/tests/login_panel_pty_e2e.rs` (openpty re-exec + mock supervisor), and the QA runner (`scripts/qa/run_suite.py`) which is the only place that composes sandbox + mock + agent + oracle in one place — in Python | **partial** | Every new end-to-end pty test re-invents the composition | covered by items 4 + 11 | medium |

### 1.7 The script-consumption gap (detail for item 3)

The faux provider serves a finite queue and, when empty, answers a `stopReason: error` message
"`No more faux responses queued`" (`crates/pa-ai/src/providers/faux/mod.rs:743-790`). It exposes
`get_pending_response_count` (`:250`) and `call_count` (`:211`), and `FauxProviderRegistration` is
`Drop`-less — `unregister()` (`:266`) is an explicit call. In-tree, `get_pending_response_count` is
used **only inside `pa-ai`'s own tests** (`crates/pa-ai/src/providers/faux/tests.rs:250,285,314,379,401`);
no `pa-cli`/`pa-core` test asserts that a script was fully consumed. Under-consumption therefore
cannot fail a test — it is silently dropped work, which Grok's `mock_server.rs:1169` equivalent
("double fails the test when never exercised") closes. On our side the same hole exists twice: the
faux queue and the `MockInferenceServer` queue (`queued()` at `mock_server.rs:280` is asserted only in
the module's own tests, `:672,685,695`).

### 1.8 The HTTP-double sprawl (detail for items 1, 4, 8)

27 test files hand-roll a loopback HTTP server (15,336 lines). Four of them re-implement the same
"serve scripted SSE" loop: `crates/pa-daemon/tests/thinking_level_e2e.rs:92`,
`queue_delivery_e2e.rs:194`, `compaction_summary_stream_e2e.rs:143`,
`provider_failover_e2e.rs:91`. Two crates already have a *named* shared helper with the same body:
`crates/pa-core/tests/common/mod.rs:14` (`MockServer::start_scripted`, `Scripted::{Response,Delayed,Hang}`)
and `crates/pa-models/tests/common/mod.rs:21` (same shape, plus TLS). Neither is visible outside its
crate, and neither is the feature-gated `MockInferenceServer` that does more.

### 1.11 Process ownership (detail for TestProcessTree)

- `PDEATHSIG` is armed in exactly two test files: `crates/pa-cli/tests/windows_runtime_triage_e2e.rs:177`
  and `crates/pa-cli/tests/interactive_daemon_e2e.rs:415`.
- Test spawn guards kill **the direct child only**: `compaction_outcome_e2e.rs:43-47`,
  `fork_isolation_e2e.rs:33-38`, `worker_stderr_e2e.rs:19`, `supervisor_restart_e2e/restart.rs:93` —
  `child.kill(); child.wait()`. A SIGKILLed test process therefore leaks the daemon's workers,
  which is precisely the failure Grok's `PtyHarness` (`pty.rs:131`, `pty_spawn.rs:192`) prevents.
- Group kill exists but only in two places: `crates/pa-daemon/tests/persistent_shells_e2e.rs:23,59`
  (`killpg`) and the product-side helper `pa_core::platform::process::set_new_process_group`
  (`crates/pa-core/src/platform/process.rs:27`, used at `pa-cli/src/interactive_mode/daemon.rs:186`,
  `tools/bash_local.rs:62`) — never in a test teardown.
- `pa-perf`'s harness sets `setsid` (`pty.rs:75`) but signals one pid (`finish:267-289`).

So: **partial** — the mechanism exists in the product crate and in two tests; the *ownership rule*
("a child you spawned is torn down by pid, group-bounded, and pdeathsig-armed") is not enforced
anywhere in the test tree.

---

## 2. Already there — do not build it again (verified against this tree)

Every row below was re-checked by reading the file, not by trusting the earlier brief or the
earlier version of this report. Where the earlier version was **wrong**, it says so.

| item | what we actually have | verdict |
|---|---|---|
| **Mock inference server (HTTP)** | `crates/pa-ai/src/testing/mock_server.rs` — **778 lines**: real `TcpListener`, HTTP/1.1, `chunked`, `text/event-stream` ×4, and queue modes `queue_response / queue_json / queue_sse / queue_sse_dropping / queue_sse_truncated / queue_sse_raw / queue_hang / queue_sse_stalling`, plus `RecordedRequest` with `header()` and `body_json()`. **Stronger than the brief described.** | **have** |
| **SSE failure modes** | the same file: dropping (cut mid-stream), truncated, stall, hang, raw frames. This is the capability the earlier version of this report got wrong — it claimed we had none. | **have** |
| **conversation_script DSL** | `crates/pa-cli/src/print_runtime/faux.rs`: `PRIME_AGENT_FAUX_SCRIPT`, per-child provider keyed by child id, step queue. Scripts *responses*, not byte streams, and has **no failure setters** (no stall/refuse/cut/doom-loop). | **partial** |
| **TestSandbox** | 11 definitions across 4 files in `pa-cli/tests/` (`fn sandbox(` ×4 plus home helpers), each scrubbing a **different** variable set. | **partial** |
| **Headless runner** | `--print` / `--mode json`; no timeout scaling. | **partial** |
| **PTY harness** | `crates/pa-perf/src/bench/pty.rs` — `PtyChild`, `openpty`+`setsid`, `vt100::Parser`, `wait_screen_contains`, `wait_screen(settled)`, `screen_text()`. **`pub(crate)`**: not reachable from `pa-cli/tests`. | **partial** |
| **Mock OTLP collector** | `rg 'otlp|opentelemetry' crates/` → 0 exporters. Nothing to mock. | **not needed** |
| **Mock managed-config server** | no managed-config/policy sync exists in this tree. | **not needed** |
| **ACP stdio client** | no ACP implementation. | **not needed** |

### 2.1 Two corrections to the earlier version of this report

**(a) It said we had no HTTP provider double. That is false.** `MockInferenceServer` exists with
SSE and five failure modes. What it *lacks* is the other half of grok's design:

- **no path routing.** `serve()` (`mock_server.rs:338-356`) pops one FIFO queue per request; it
  parses the request line but never dispatches on the path. Grok routes
  `InferenceEndpoint::ChatCompletions / Responses / Messages` separately
  (`mock_server.rs:858-867`), so a test there can hold three independent expectations. Ours
  cannot: a `/v1/messages` request and a `/v1/chat/completions` request consume the same queue in
  arrival order. **This is the 40-line change worth making** (item §3.3), and it is a real
  limitation, not a missing feature.
- **adoption is 2 test files.** `rg -l MockInferenceServer crates/` → `provider_session_id_wire.rs`
  and `provider_failover_drill_e2e.rs`. Of the five failure modes, the drop path is used twice and
  `queue_hang` / `queue_sse_stalling` are used **nowhere**. The capability exists and is barely
  exercised — a defect class of its own, and cheaper to fix than to build anything new.

**(b) It counted `Duration::from_secs` as 191 in `pa-cli/tests`.** Actual: **151** in
`pa-cli/tests`, **655** across all `crates/*/tests`. The conclusion (a scaled-timeout helper is
cheap and worth it) survives; the number did not.

## 3. Absent or partial, and worth taking — ascending by price

Ordered by cost, and every "why" cites something measured in §2. The first two exist because the
earlier version of this report **overstated** what we had.

### 3.1 Path-routed mock responses — ~40 lines, no production change

`MockInferenceServer` pops one FIFO queue (`mock_server.rs:350-356`) and never dispatches on the
path. Grok routes `ChatCompletions / Responses / Messages` separately
(`mock_server.rs:858-867`), so a test there holds three independent expectation queues.

Our cost: a `HashMap<&'static str, VecDeque<ScriptedResponse>>` keyed by request path, with the
existing global queue as the fallback for a path a test does not name. **Effect:** the retry and
dialect-switch paths become expressible — today a test cannot say "the next `/v1/messages`
request stalls while `/v1/chat/completions` succeeds".

**Red-first proof:** queue one response for `/v1/chat/completions` and one failure for
`/v1/messages`; today both requests pop the same queue in arrival order and the test cannot
distinguish them.

### 3.2 Scaled-timeout helper — ~10 lines, 0 risk

**151** `Duration::from_secs` literals in `pa-cli/tests` (**655** across all `crates/*/tests`), and
the gate already moves 5 units to a serial tail because they are timing-sensitive. Grok's
`GROK_TEST_TIMEOUT_SCALE` exists for the same reason.

Cost: `fn scaled(d: Duration) -> Duration { d.mul_f64(env_f64("PRIME_AGENT_TEST_TIMEOUT_SCALE", 1.0)) }`
plus replacing literals incrementally. **Effect:** a slow CI box stops producing false reds, which
is the failure mode the serial tail documents today.

### 3.3 Exercise the SSE failure modes we already have — 0 lines of new capability

The mock can `queue_sse_dropping`, `queue_sse_truncated`, `queue_hang` and `queue_sse_stalling`.
Measured usage: dropping/truncated twice (`provider_failover_drill_e2e.rs`), and `queue_hang` /
`queue_sse_stalling` in **no test at all**. This is the cheapest item on the list and the one the
earlier report could not see, because it assumed the capability was missing.

**Red-first proof:** a test that stalls after the first chunk must observe a bounded retry; write
it against `queue_sse_stalling` and watch it fail if the retry bound is removed.

### 3.4 Script-consumption guard — ~1 day, no production change

Verified: `rg 'Drop for|unconsumed|script_violations' crates/pa-cli/src/print_runtime/faux.rs` → 0.
`get_pending_response_count` is used only inside `pa-ai`'s own tests; `queued()` only in the mock
server's. So a scripted test that stops exercising its third turn stays green.

**Effect:** kills the "test that cannot fail" class inside every mock-driven test. Grok's version
(`assert_no_script_violations` + `Drop`, guarded by `!std::thread::panicking()` so a real failure
is not masked) is the shape to copy.

### 3.5 Roster-wide tool-definition smoke — ~0.5 day

The TS-differential schema corpus covers **3** tools (`schema.json` → `caseCount: 3`, naming
`bash`, `edit`, `ipython`), and the fork-native tools (`lsp`, `graph`, `edit_hybrid`, `jev`) are
iterated by no test. `params_schema()` exists in exactly **4** files. A duplicate `depth` key was
already caught by hand in `graph.rs`, which is the class this would catch mechanically.

**Effect:** closes "documented, never wired" — the defect class `AGENTS.md` ranks first.

### 3.6 Process-tree teardown in test harnesses — ~0.5 day

`PDEATHSIG` appears in one test (`windows_runtime_triage_e2e.rs:177`) and the shared PTY harness
does not arm it. Grok's `orphan_reap.rs` arms `PR_SET_PDEATHSIG(SIGKILL)` at spawn and documents
the residual gap (a SIGKILLed runner leaves grandchildren).

**Effect:** a CI timeout no longer leaves orphans accumulating on the host.

### 3.7 Telemetry canary over the bytes we send — ~0.5 day

We have the policy (`properties.rs` rejects non-primitives) and a key-dropping sanitiser
(`client.rs:396,457`), and the test `flush_sanitizes_events_against_the_catalog` proves an
**unknown key** is dropped. Nothing proves a **secret inside an allowed value** is refused:
`free_string(64)` admits 12+ properties of arbitrary text, and `rg 'canary|sha256' crates/pa-telemetry/`
→ 0.

**Effect:** the privacy contract becomes observable at the byte level rather than argued from types.

### 3.8 One `TestSandbox` — ~1 day

Measured: **11** definitions across **4** files in `pa-cli/tests`, and they scrub **different**
variables:

- `package_resources_e2e.rs:66-68` removes `PRIME_AGENT_SESSION_DIR`,
  `PRIME_AGENT_CODING_AGENT_SESSION_DIR`, `PI_OFFLINE`
- `packaged_layout_e2e.rs:197-202` removes `PI_PACKAGE_DIR`, `PRIME_AGENT_KERNEL_PYTHON`,
  `PRIME_AGENT_KERNEL_VENV`
- `differential_cli.rs:331-344` **sets** `HOME`/`PRIME_AGENT_CODING_AGENT_DIR`/`TMPDIR`/`PI_OFFLINE`
  and **removes nothing** — so an ambient `PRIME_AGENT_SESSION_DIR` survives into the child

That last one is not hypothetical: `RT-007-launcher-env-isolation` is **known-red-on-purpose**
(`qa-tasks/README.md:96-97,265`) because the launcher does not pin `PRIME_AGENT_SESSION_DIR`, so an
ambient value escapes the isolated home. One sandbox owner would make every suite inherit the same
scrub list, and the RT-007 gap would be visible from any test rather than one.

### 3.9 Shared PTY harness — ~2-3 days

`PtyChild` is `pub(crate)` to `pa-perf`, so `pa-cli/tests` cannot use the one screen-decoding
harness in the tree; **9** files re-declare `HARNESS_LOCK`, and the PTY fixes are effectively made
per file. Extract to a dev-only crate (`pa-test-pty`) with `wait_until`, quiescence, and an
artifact dump on failure.

## 4. Absent, and we do NOT need it

| Grok piece | Why not |
|---|---|
| **Mock OTLP collector** (`mock_otel_server`, `wait_for_silence`, header asserts) | We have no OTLP exporter: `rg 'otlp\|OTEL_EXPORTER\|opentelemetry' crates/pa-telemetry/` → 0 hits, and no `opentelemetry` crate in `Cargo.toml`/`Cargo.lock`. A collector would verify a protocol the product does not speak. The property it protects (nothing sensitive leaves) is reachable over our real wire instead: `analytics_wire.rs:163` already serves the real `AnalyticsSink` HTTP request and asserts the exact body and that `authorization` is absent (`:74`). Add the canary there (§3.7). Build a collector only if an OTLP exporter ever ships. |
| **Mock managed-config / requirements server** | Nothing to mock: no managed config, no org policy, no `requirements.toml` sync. `rg -i 'managed|requirements\.toml' crates/` returns only unrelated hits — `homebrew::managed_kind` at `crates/pa-cli/src/installer_update.rs:94`, the *managed install root* of the updater (`crates/pa-cli/src/update_flow/swap.rs:177`, `crates/pa-core/README.md:76`), and heartbeat "managed job" rows (`crates/pa-tui/src/heartbeats_picker/tests.rs:453`). Nothing reads a remote policy file. Settings are purely local (`crates/pa-core/src/settings/types.rs`, 731 lines). A mock would test a feature that does not exist. |
| **`conversation_script` DSL as a DSL** | Our faux script already covers the semantics (content blocks, `stopReason`, `errorMessage`, `delayMs`, `repeatLastResponse`, per-child lanes — `script.rs:39-157`). What is missing is *consumption enforcement* (§3.2), not syntax. A builder DSL would add a second grammar to keep in sync with the JSON one that `qa-tasks/*/faux-script.json` already ships as data. |
| **150-line `PtyHarness` façade with 20 wait primitives** | We have the primitives (`pty.rs:117-289`) and 17 specialised pty suites whose *oracles* are terminal-state differentials (`terminal_state_differential_e2e/ledger.rs`), kitty probes (`kitty_early_typing_e2e.rs:35-42`), termios deltas (`terminal_state_differential_e2e/harness.rs:328-380`). Porting Grok's façade without its consumers would create a third pty vocabulary. Take the *shared home* (§3.6), not the surface. |
| **`alacritty_terminal` screen model / styled-run asserts** | `vt100` 0.15 (`crates/pa-perf/Cargo.toml:21`) cannot do styled runs; but no failing oracle in this tree needs them — the styled golden that exists is a Rust-side `Style` snapshot on a pure widget (`crates/pa-tui/src/onboarding.rs:576`), no terminal involved. Defer until a real highlight bug escapes. |
| **YAML scenario layer (44 actions)** | `qa-tasks/task.yaml` is already a declarative scenario format with a strict mini-YAML parser (`scripts/qa/run_suite.py:240`), 4 lanes, budgets, state oracles and transcripts. A second YAML dialect for the TUI would duplicate it. If TUI steps are needed, add step verbs to `qa-tasks`, not a new file format. |
| **Their `#[ignore]`-gated pty suite as the default shape** | We already have 80 `#[ignore]` attributes and a gate that shards and serializes (`prepush-gate.sh:263-271`). Copying the gating is churn; the useful part (scale knob, §3.1) is 10 lines. |

---

## 5. The three biggest effects per unit of cost (revised after measurement)

1. **Exercise the SSE failure modes we already own (§3.3) — hours, no new code.** The mock can
   stall, drop, truncate and hang; `queue_hang` and `queue_sse_stalling` are used by **no test**.
   The capability is written and unproven, which is strictly worse than absent: it looks like
   coverage. First action in this whole report, and it was invisible to the earlier version because
   it assumed the capability did not exist.
2. **Path-routed mock responses (§3.1) — 40 lines.** Without it a test cannot hold two dialects
   apart, so the retry/dialect-switch paths have no fixture. This is the one real gap between our
   mock and grok's, and it is small.
3. **One `TestSandbox` (§3.8) — 1 day.** Four copies scrub four *different* variable sets, and
   `differential_cli.rs` scrubs none while setting `HOME`; `RT-007` is known-red-on-purpose for
   exactly this class. The proof that the defect is live is already in the corpus.

Honourable mentions at the same price: the scaled-timeout helper (§3.2, 10 lines) and the
script-consumption guard (§3.4, 1 day).

**What the earlier version ranked first — the roster-wide tool-definition smoke (§3.5) — stays,
but third-priority rather than first**: it is real (`caseCount: 3` of 5 tools, `params_schema()` in
4 files), yet it catches a modelling defect, while §3.3/§3.1 leave *retry and dialect correctness*
untested, which is the class the provider lane exists for.

## 6. Method, and what is still unverified

**How this revision was produced.** The earlier version of this file (and the brief it was built
from) contained claims I could not let stand, so every row in §2 and §3 was re-measured with `rg`
and by reading the file:

| claim | earlier version | measured |
|---|---|---|
| we have no HTTP provider double | asserted | **false** — `pa-ai/src/testing/mock_server.rs`, 778 lines, HTTP/1.1 + chunked SSE + five failure modes |
| that mock routes by path | implied (compared to grok's) | **false** — FIFO queue, no dispatch (`mock_server.rs:350-356`) |
| the mock is widely used | not stated | **2 test files**; `queue_hang` and `queue_sse_stalling` used **nowhere** |
| `Duration::from_secs` in `pa-cli/tests` | 191 | **151** (655 across all `crates/*/tests`) |
| sandbox copies | 4 | **11 definitions in 4 files**, with different scrub lists |
| brief's line citations for grok's README | `:25, :32, :39` | **`:26, :33, :40`** — every one shifted +1; `:89` was exact |

**Verified against grok's real source** (`gh api`, not the brief): `mock_server.rs` (1184 lines)
routes `InferenceEndpoint::ChatCompletions / Responses / Messages` separately at `:858-867`;
`sse` builds three endpoint formats (README L39); `TestSandbox` owns
`HOME/USERPROFILE/GROK_HOME` and clears env (README L40); the consumer matrix is at README L89.

**Still unverified, and deliberately so:**
- Whether grok's `conversation_script` failure setters are equivalent to the SSE queue modes we
  have. Both exist; I did not run either.
- grok's `headless.rs:14-32` timeout implementation — I read the README row (L42), not the file.
- Whether `queue_hang` actually hangs in a way a test can bound: the API exists, no caller
  exercises it, and "unused" is not the same as "working".
- The PTY-harness extraction size (§3.9, 2-3 days) is an estimate from 425 lines plus 9 lock
  declarations, not a measured change.

**Rule this file now follows:** a claim about *our* tree carries a `file:line` that I read; a claim
about grok carries a `gh api` path I fetched. Anything else is marked here rather than asserted.
