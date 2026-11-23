# Agent errors — the classes we keep repeating

One entry per class, not per incident. An entry stays until the class stops recurring.

## 1. A test that passes on the broken code

**Seen:** three times in one session (2026-10-23).

- A per-file write queue was added to close a lost-update race; the test passed with the queue
  and without it, because `std::fs::write` never interleaves bytes — the hazard was the read, not
  the write. The right fix was `execution_mode() = Sequential`.
- A demotion test asserted `confidence < 0.95` for an import whose module *did* match, where 0.95
  is correct. It would have passed forever and proved nothing.
- A test exercised `with_timeout` directly to "prove" that `goto` bounds its preparation. It never
  touched `goto`.

**Caught by:** temporarily inverting the fix and requiring red.

**Rule:** a test must go red with the fix removed. If it does not, it is documentation at best.
Write it after the fix, invert, watch it fail, restore.

## 2. Corrupting a file by rewriting it instead of editing it

**Seen:** twice (2026-10-23). A Python `write_text` replacement dropped test stdout into
`store.rs` and later `edit_hybrid.rs`; the files became 3 and 1209 lines of ANSI escapes.

**Caught by:** `cargo clippy` (`unknown start of token: \u{1b}`) and a line count.

**Rule:** edit existing files with `edit` and an exact old/new string. Never construct a whole
file body in Python. After any programmatic write, check the first line and the line count against
what you expected.

## 3. Proposing a fixture that cannot express the claim

**Seen:** once (2026-10-23). A bare repository was proposed as "a directory outside any
repository"; `rev-parse HEAD` in a bare repo exits 0 and prints `HEAD`, so the function under test
reported a context instead of `None`.

**Caught by:** a subagent measuring the actual command instead of trusting the proposal.

**Rule:** before choosing a fixture, run the exact commands the code under test runs, in that
fixture shape, and look at the exit codes.

## 4. Trusting a document over the tree

**Seen:** repeatedly. `graph.auto = "watch"` was documented in three places and never wired;
the design matrix's `prime_now` column read "НЕТ" for features that exist; the `.research` index
listed reports that are not in a clone.

**Caught by:** reading the consumer, not the promise.

**Rule:** a claim about behaviour is verified against the code that would have to implement it.
Documents describe intent and drift; the tree is what runs.

## 5. Python string surgery on large Rust test files

**Seen:** once (2026-10-24). Inserting a regression test into `installer_stop_gate.rs` by
Python string replace spliced the new test's doc block into the middle of another test's
`#[test]` + fn pair — orphaning the ancestor fn, duplicating the test name, and (after a
failed `git checkout --` recovery attempt) landing the corruption in a commit that failed
the workspace clippy gate at push time.

**Caught by:** the workspace clippy gate at push (duplicated test name, unused fn).

**Rule:** inserting into a large file with many `#[test]` blocks goes through the edit
tool with exact unique anchors; Python replace is for single-site replacements whose
anchor was read from the file in the same step. After any multi-block edit, run
`cargo check -p <crate> --all-targets` before committing; a compile check catches the
splice immediately.

**Related:** `git checkout --` on a file carrying uncommitted work destroys that work —
the 2026-10-24 recovery of `installer_stop_gate.rs` lost only self-written code, but the
rule (never `checkout --`, `stash`, `reset --hard`) exists because the next file might
not be yours alone.

## 6. Reading a red suite as a code failure when the disk was full

**Seen:** once (2026-10-31). `cargo test -p pa-graph --lib` reported 102 failures in 0.5 s
across unrelated modules (`coverage`, `deps`, `schema`, `scoped_context`). Every one was
`tempfile` raising `StorageFull`; the root filesystem was at 100 % because another agent
had left dozens of `/tmp/rev3*` checkouts behind. Nothing in the crate had regressed — the
same suite was green minutes earlier and green again after the space came back.

**Caught by:** reading one panic message instead of the failure count.

**Rule:** a suite that fails wholesale in under a second is an environment failure, not a
product one. Read the first panic; if it mentions the disk, check `df -h /` before
touching code. Freeing someone else's scratch is not yours to do — say so and wait, or
move your own run under `TMPDIR=<repo>/.tmp` as the gate does.

## 7. A fixture that cannot distinguish the wrong implementation

**Seen:** six times (five in one session, 2026-11-06; the sixth on 2026-11-08), each caught by
an independent verifier rather than by the author.

- `cycle_model` must filter judgment-only dialects. The fixture put `subq/snap` LAST in the
  catalog, so "step the unfiltered list and wrap" and "filter only the returned model" both
  produced an acceptable answer. Moving the trap between two chat models made all three
  implementations distinguishable.
- The startup flush must not be joined before the run. The guard read the source and forbade
  `.await` at the binding — hoisting the bounded join above `run.await` passed it. The fix was
  structural: make the join TAKE the run's result, so the hoisted call cannot be written.
- The alias repair must handle a group. The fixture had one `as` per line, so a scan that
  stopped at the first one passed.
- The credential redactor must cover bare values and fragments. Neither shape had a fixture
  until the verifier named them.
- The socket directory must be private. A mode-only check admitted a 0755 directory owned by
  another user — the fixture only ever created directories we owned.
- The differential runner must not pin a fixed port. A re-grade verified the guard's assertions
  existed and named the right properties; the mutation run then showed the fixed-port revert
  SATISFIES `contains("REPLAY_PORT=")` — the `sed` parser emits that line regardless, and
  `PA_DIFF_SELFTEST` in a usage comment satisfied `if false`. Existence of an assertion is not
  its discriminating power: run the mutation, in review too. Fixed same day — the assertions now
  name assignments/branches and a started-with-resolved-port check, 5/5 mutations caught
  (`c1d8aac0a`).
- The fork-surface guard's SELF-TEST never ran its own baseline (seventh, 2026-11-09,
  `5a4152b31`). It copied a hand-kept list of files into a temp fixture, injected three breaks,
  and counted any red run as detection — but the list omitted `pa-edit/src/tool.rs`, so the
  fixture was ALREADY red on the clean state, and every "detected" break was detected by the
  same unrelated failure. Measured: no mutation at all, exit 1. Break 2 also removed
  `tools/graph.rs`, which has not existed since the graph became a directory, so it injected
  nothing. Three rules came out of it: build the fixture from the same names the checks read,
  require the CLEAN fixture to be green first, and match each injected break against the TEXT of
  the check it targets — plus refuse a mutation whose pattern no longer matches.
- Four QA task oracles graded the FILE or the TRANSCRIPT instead of the answer (eighth,
  2026-11-09, `77b7407d4`). Measured on the shipped ST-003 oracle: describe untouched, summary
  corrupted, the required marker put in a COMMENT — exit 0, the oracle passed a wrong answer.
  ST-002 searched its completion marker over the whole transcript, where the task's own prompt
  supplies it; ST-001 accepted the two caller names in any relationship; ST-004 accepted DELETE
  even when the typed tool answered KEEP. Each is now a graded pair (wrong outcome red, right
  outcome green) in `scripts/qa/test_st_oracles.py`, and the four oracles went red on the first
  run of it — 12 of 18 cases.

**Caught by:** writing the wrong implementation on purpose and running the test. Three of the
first five passed on first contact.

**Rule:** for each guard, name the wrong implementations it must reject, put one of them in the
fixture's way, and run it. A test that has only ever been run against the correct code is a
hypothesis. This is the same rule as §1 with the emphasis moved: §1 asks "does it fail on the
OLD code", this one asks "does it fail on the PLAUSIBLE code". And when a guard has a self-test:
the UNMUTATED fixture must pass first, or the self-test is measuring its own fixture.

## 8. A fix that makes the common path pay for the rare case

**Seen:** once (2026-11-06), caught by a measurement that did not finish.

The pa-graph alias repair read and PARSED every file holding an unresolved call site on every
incremental pass — 1756 of 2250 files in this repo. The repair itself was right, and every test
passed; a run on the real tree did not complete in 25 minutes. Two changes fixed it: an empty
pass returns immediately (nothing arrived, nothing left ⇒ no verdict can change), and the
bindings come from a text scan instead of a parse. Measured after: 46 ms with the guard, 64 ms
without, over 400 files.

**Caught by:** trying to measure the real tree and noticing that "no result" is a result.

**Rule:** a repair that runs on every pass is a cost on every pass. Measure the COMMON case
(nothing changed), not the case the fix was written for. If the measurement does not finish,
that is the finding.

## 9. A fix that leaks past the operation it was written for

**Seen:** once (2026-11-18), caught by three full suite runs and one A/B.

The daemon's kill test failed with `kill waited out the held provider response (60.31s)`, so the
abort was not reaching the turn. The window was real: the worker marks the session busy before
the agent registers its run, and an abort that landed in between found no run and did nothing.
The first fix made the abort sticky — no run, remember it and apply it to the next one. It
passed its own test and turned the kill test green.

It was wrong. A flag that survives the turn also cancels the NEXT turn, and
`abort_and_send_queued` deliberately starts a follow-up after aborting. Three consecutive full
`pa-daemon --lib` runs reported 1389/1, each time a different test of that queue family. Every
failed test was green alone, which is exactly how a regression hides behind a "flaky" label.
The decisive step was not more reasoning: it was disabling the flag's apply and running the whole
suite, 1390/0.

**Caught by:** the same family failing three times with three different names. One failure is a
flake; three different names in one family is a cause.

**Rule:** an abort, a cancel or a "skip next" must expire with the operation it belongs to. Ask
what the signal means after the operation ends — if the answer is "still pending", the state
outlives its subject and will act on an unrelated one. Prefer a generation counter scoped to the
operation being started over a boolean that outlives it, and prove the negative case (the next
operation still runs) in the same change.

## 10. Measured: what I actually get wrong (one long session, 2026-11-12..18)

Instead of guessing at my own failure modes, I counted them in the session log
(`01a1d0d1…jsonl`): 11 228 tool calls, 217 of them `edit`, 92 Rust compile
errors, 342 panic lines forming 148 distinct signatures.

**Compile errors by code** — the top of the distribution is the useful part:

| code | count | what it means |
|---|---|---|
| E0425 | 18 | cannot find value in scope: I used a name I assumed existed (`scope_key`, `entry_epoch`, a helper I had just invented) |
| E0308 | 12 | wrong type: `&str` where `String` was needed |
| E0061 / E0599 / E0382 | 8 each | wrong arity, no such method, use-after-move |
| E0560 / E0063 / E0433 / E0277 / E0609 | 6 / 6 / 6 / 6 / 4 | struct field misuse, missing field, unresolved import, trait bound, field on the wrong type |

**Edit-tool failures: 4 "No match found" and 4 "Found multiple matches" in 217
edits (3.7%).** Both are the tool doing its job; the cost is the retry.

**Productive awkwardness:** 170 log lines of `Blocking waiting for file lock on
build directory` — cargo runs overlapping, which the repository's own rule
already forbids. Each one is wall clock spent waiting on my own other build.

**Two mistakes worth naming, because they were not compile errors:**

- A variable named `p` was reused for a file path in an edit cell, clobbering the
  session-log path variable from an earlier cell. The next analysis silently read
  the wrong file and returned zeros. **Rule:** keep one name per resource across
  cells; a scratch Path is `p_<what>`, never `p`.
- Reading a file immediately after a command that writes it, when the command ran
  in a background handle. **Rule:** a file produced by a background handle is read
  only after its completion notification.

**Rule from the top row.** Before using a helper, constant, or field in a patch,
confirm it exists and is in scope at the patch site — one `rg` costs a second, one
E0425 costs a rebuild. The 18 occurrences are all the same shape: writing code
against an assumed API surface.

## 11. `cargo check` is not clippy, and a green test is not a green lint

**Seen:** twice in one session (2026-11-18/19), both times on code that had
already passed `cargo check` and its own tests.

- A new test in `pa-daemon` cloned a `PathBuf` it then dropped unused. `cargo
  clippy -p pa-project` was green; the gate's workspace clippy was not.
- A `const` declared inside a function after statements, and a `use` redundant
  with the file's imports. `cargo check` accepted both; clippy at `-D warnings`
  refused both.

**Caught by:** the pre-push gate, twice, after the commit existed — which is the
expensive way to learn it.

**Rule:** for every crate a slice touches, run
`cargo clippy -p <crate> --all-targets -- -D warnings` **before** the commit,
including crates where only tests changed. `cargo check` answers "does it
compile"; it does not answer "will the gate pass", and the difference here was
`items_after_statements` and a redundant clone.

## 12. A contract test needs three red-first probes, not one

**Seen:** once (2026-11-19), on the `docs/flaky.md` contract test — and the three
probes each found a different hole.

1. **Substring matched the file it lived in.** Searching source for `#[ignore`
   found the test's own doc comments, so the "bare attribute" check failed on a
   clean tree. Probe: does the clean tree pass? Fixed by parsing the attribute
   shape (line starts with it, followed by `]` or `=`).
2. **The checker accepted its own skip-list.** Renaming a journal row to
   `a_test_that_was_renamed` still passed, because the row also quotes its crate
   (`pa-daemon`) and the checker matched that. Probe: does a FAKE name fail? It
   only failed after crate names, flags and commit hashes were excluded by shape.
3. **Too narrow a lookup.** `fn <name>` missed `IDLE_AFTER_ABORT_BUDGET`, a
   constant the journal legitimately names. Probe: is every kind of thing the
   journal cites found? Now it is "this identifier exists", with word boundaries
   so `a_foo` is not satisfied by `a_foo_bar`.

**And the test found a real lie on its first clean run:** the journal cited a
renamed test and a reverted commit as coverage.

**Rule:** a contract test over a document needs at least three probes — clean input
passes, a fake entry fails, and every shape the document is allowed to cite is
accepted. One probe proves the test runs, not that it discriminates.

## 13. Long-lived cells and the wrong edit surface

**Seen:** 2026-11-21, while restoring repo context and fixing durable plan state.

- Nine `python_repl` cells hit the 60 s interrupt while waiting on builds or tests.
  **Rule:** start long commands as background handles, keep working on independent
  reads, and collect on the completion notification; do not `sleep()` or poll a
  build in a cell.
- The previous session logged 40 Python tracebacks, mostly cheap type/name errors
  in scratch code. **Rule:** assign typed results to variables and check the
  daemon store's shape (`SessionEntry.fields` is flattened JSON) before writing a
  test oracle against `pa_types::FileEntry`.
- The user had to name the edit surface: edits go through the native `edit` engine
  (`replace`, `apply_patch`, `hashline`), and Python is for reading, searching, and
  parsing. **Rule:** a file change is an `edit` call; `cargo fmt` is the only
  command that rewrites.
- Three `graph trace` failures named `project_store::ProjectRecorder::sync`; the
  suffix resolver was fixed in fork.020. **Rule:** after `graph.search`, use the
  resolved path; a failed trace is not proof the symbol is absent.

## 14. A PATH tool that is a shim for a toolchain without the tool

**Seen:** 2026-11-21. `lsp.find_referencing_symbols` failed with the opaque
`initialize failed: service stopped`, then backed rust-analyzer off for 180 s.
The binary on `PATH` was the rustup shim; the active toolchain (1.99.1) did not
ship `rust-analyzer`, while `stable` and `1.99.0` did. The spawn error was a shim
message ("Unknown binary") that the LSP client discarded on `stderr = null`.

**Caught by:** running `rust-analyzer --version` directly, then scanning
`~/.rustup/toolchains/*/bin/rust-analyzer`.

**Rule:** resolve a tool by what the current environment actually provides, not
by the first name on `PATH`. For rustup tools, check `RUSTUP_HOME`, the active
`$HOME/.rustup`, and the real home's `.rustup` when `HOME` is isolated; keep a
clear PATH fallback. A child process's stderr is diagnostic evidence, not noise:
pipe it into the spawn error instead of discarding it.

## 15. A test I wrote asserted the wrong string shape, twice

**Seen:** 2026-11-21, the `[limits]` slice. Two self-inflicted failures, both in
tests I had just written.

1. `truncate_tail` splits on `'\n'`; a stream that ends with a newline yields a
   final EMPTY element that consumes one `max_lines` slot. With `max_lines = 3`
   over `seq 1 10` the kept text is `"9\n10\n"`, not `"8\n9\n10"`. My first
   assertion expected the second.
2. The same off-by-one hit the unit test: `max_lines = 3`, `max_bytes = 50` over
   100 generated lines keeps two visible lines, not three.

**Caught by:** running the test and reading the actual `left`/`right`, then
simulating the exact algorithm in a REPL cell before rewriting the assertion.

**Rule:** when an assertion fails on a string shape, reproduce the function's
algorithm on a tiny input in the REPL and read what it returns; do not re-guess
the expected string. A line-counting test must account for the trailing empty
element (`n` visible lines need `max_lines = n + 1` when the stream ends with a
newline). Record the shape as a memory so the next slice does not repeat it.

**Also seen in the same slice:** a clippy `format_collect` from
`.map(|n| format!(...)).collect::<String>()` — `writeln!` into a `String` is the
form the lint wants. Run clippy on the new test code, not only the production
code.

## 16. A `grep` wrapper on `PATH` inverted the gate's own verdicts

**Seen:** 2026-11-22, the fork.025 release gate. `scripts/fork/prepush-gate.sh`
went RED on its own contract tests (6 failures in `scripts/test_prepush_gate.py`)
although neither the gate nor its tests had changed in the merged tree.

**Cause:** this machine's `PATH` puts `/root/.local/bin` before `/usr/bin`, and
`/root/.local/bin/grep` is a transparent `grep` -> `rg` wrapper. The gate is
written for GNU grep: `grep -c '^ok ' file` must print `0` when nothing matches,
and the wrapper printed nothing and exited non-zero instead. Every count in the
summary (`ran_ok`, `ran_skip`, `ran_no_tests`, `skipped`) read as empty, so the
gate printed `(1 ok,  skipped:  with no tests to run)` and its tests failed on
the shape. With `rg` absent from `PATH` entirely, `check-fork-surface.sh` lost
every `FORK-ONLY` check and reported 9 false failures.

**Caught by:** reading the failing assertion's `left` value, noticing the empty
`grep -c` field, then `which -a grep` and reading the wrapper script.

**Rule:** a gate's shell must not depend on which `grep` the caller's `PATH`
resolves first. Two facts must hold before trusting a gate verdict on this
machine: `grep --version` is GNU grep, and `rg` is reachable. A wrapper that
changes `-c` semantics turns a measurement into a guess, and the gate reporting
a false RED costs the same trust as a false GREEN. When a shell assertion fails
on an empty field, print the field's producer before believing the assertion.

## 17. A verification lane that skips is a lane that reports success

**Seen:** 2026-11-23, restoring verus, loom, kani, miri, ASan and shuttle. Every
one of the five was either absent or skipping, and the skip path exited 0.

**Cause:** each lane probed for its tool in ONE place, and each probe was written
against whatever path that day's install happened to use:

* `scripts/verus-check.sh` and `verus-mutants.sh` looked at `verus/toolchain`;
  the documented install (`verus/README.md`) puts the release under
  `/root/verus-pilot/verus-x86-linux`. Both exited 0 having verified nothing.
* `kani-check.sh` accepted a partial bundle, then failed at the first crate
  because `KANI_HOME` was resolved from a different root than the one the driver
  was invoked from.
* The `loom` feature existed as a cargo feature but no `--cfg loom` was emitted
  (`build.rs` missing), so the module compiled to nothing: `cargo test --features
  loom` collected zero tests and printed `ok`.
* `shuttle` and `miri` ran, but through their oldest entry point: `check_random`
  makes no guarantee about what it missed, and a single Miri run proves one
  incidental alignment and one thread schedule.

**Caught by:** running each lane and reading its output instead of its exit code.
`ok. 0 passed; 123 filtered out` and `skipping (see README)` are the two shapes
that look green and mean nothing.

**Rule:** a verification lane must print what it actually verified (counts, not
just a verdict), and it must state the tool locations it probed when it skips. A
skip is not a pass. When a feature flag gates a test module, prove the cfg was
emitted by asserting a non-zero test count — a `0 passed` line is the failure.
Two more consequences follow, both measured here: the version of the tool and
the version of its companion crate must be pinned together (`vstd` two weeks
behind the toolchain turned a proof-depth campaign into "a real survivor"), and
the crate the lane covers is chosen by where the risk is, not by which crate was
easy to add (ASan covered `pa-graph`, which has zero `unsafe` blocks, and
omitted `pa-mem`, which has ten).

## 18. I deleted the test that disagreed with my fix

**Seen:** 2026-11-23, the fork.025 compaction work. `auto_compaction_due` announced a
threshold compaction whose `prepare_compaction` answered `TooShort`, which read as noise: a
start/end pair ending in a skip warning on every boundary of a short session. I added a
pre-check so the trigger would stay silent, then found
`threshold_skip_records_the_durable_outcome_row` asserting the announcement, and **replaced
it** with `a_prefix_only_crossing_does_not_announce_a_threshold_compaction`.

**Cause:** the announcement is a contract, not noise. Two independent suites pin it:
`acp_mode_e2e::acp_threshold_auto_compaction_publishes_the_compaction_meta` asserts the
single-turn case publishes a non-empty list of `compaction` metas with empty payloads, and
`print_runtime_e2e` pins the user-visible text "Auto-compaction skipped: Session is too short
to compact — try again once it grows". Both were green on base `eab651262`. My pre-check broke
the ACP one in 0.9 s.

**Caught by:** the full gate, not by review, and only after I had already pushed the release
tag. The second attempt at a fix (`try_lock`, then a 50 ms bounded poll) was me still treating
it as a lock problem; the ACP test fails with a plain `.await` too, so the verdict was the
defect and the lock was never involved.

**Rule:** a test that encodes a documented, user-visible behaviour is not an obstacle to route
around — replacing it makes the suite agree with the change instead of with the product. Before
suppressing an announcement, a warning, or an event as "noise", grep for the tests that assert
it and read what they say about the contract. If the answer is "this is expected", the fix is
wrong, however clean the reasoning that produced it. The correct move here was to revert both
halves (code and test) and re-measure: 20/20 abort+threshold, the acp case green in 1.46 s,
held-turn tests back to 31.8 s, and `pa-core` compaction 137/137.
