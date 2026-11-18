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

