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
