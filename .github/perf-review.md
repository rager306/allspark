# Performance review: what to measure, where, and when

Issue #14 asked for a PR-level perf procedure, plus a first measurement of
`pa-mem/src/shard.rs` (RwLock vs version locks on the read path).

**That measurement cannot be taken, and the reason is the first entry in this
document.** Recorded here rather than silently skipped, because the rule the
issue is reaching for — *measure before you argue about locks* — has a
precondition: something must already call the code.

---

## Rule 0: there is nothing to measure until something calls it

Measured 2026-10-27: `pa_mem` appears in no crate's `[dependencies]`. The only
files outside `crates/pa-mem/` that mention `pa_mem` are its own README and doc
comments. `ShardedMap`, `RobinHoodMap`, `SpinLock`, `SixBytePtr`, `EntryArena`,
`BlockSnapshot` — none of them is reachable from the product.

So a shard.rs benchmark would measure a structure no session ever builds, and a
"version locks are 30% faster" result would justify a change to code that has no
caller. The honest order is:

1. **Adopt or delete.** Either a product path starts using `ShardedMap`
   (the fork's stated purpose for pa-mem was pogocache-style storage under the
   graph cache), or the crate is dead weight and its audit budget is better
   spent elsewhere. This is a decision, not a measurement, and
   `docs/agent-errors.md` records the class: a benchmark of unused code is a
   cost with no effect.
2. **Only then measure** with the procedure below, on the adopted path.

The same rule applies to every candidate in this document: a measurement names
the caller it serves, or it does not get taken.

---

## What we DO have to measure (and where the numbers already are)

| surface | instrument | where the number is recorded |
|---|---|---|
| launch → first paint | `crates/pa-perf` pty bench | `AGENTS.md` ("Performance and the critical path"), regression test `the_startup_flush_never_blocks_the_first_frame` |
| graph index warm path | in-tree measurement in `IndexOptions::respect_gitignore` doc | 2,058 files ~300 ms lean vs 37,131 ~2.3 s wide |
| prepush gate wall clock | `scripts/fork/prepush-gate.sh` | 3,232 s over 229 units → 1,162 s over 280 (one build instead of one per unit) |
| Miri lane | `.github/workflows/miri.yml` | pa-types platform subset 3.6 s, pa-core platform::shell 1.9 s |
| Kani lane | `scripts/kani-check.sh` | 5 harnesses, 109 checks, ~1.5 s |
| Verus lane | `scripts/verus-check.sh` | 4 mirrors, 28 obligations, 5.6–6.6 s |
| mutation lane | `scripts/verus-mutants.sh` | 16 sampled mutants, ~40 s, 100% kill |

A number with no before/after is a claim, not a result. Every row above names
the command that produced it.

---

## Procedure for a new measurement

### 1. State the question as a decision it would change

Bad: "how fast is the LSP client roster?"
Good: "does `ensure_client`'s per-key gate make opening N projects scale with N,
or serialize behind the slowest spawn?" — the answer chooses between the current
design and a sharded one.

If no decision hangs on the number, stop. `AGENTS.md` "Measure before
optimising" says the same thing with the cost attached.

### 2. Take the baseline FIRST, on the machine that will run it

```sh
cd <repo>
# free space and memory before a heavy step; refuse with a number when short
df -BG --output=avail . | tail -1
free -g | head -2
# one command, timed
/usr/bin/time -v <the command> 2>&1 | rg 'Elapsed|Maximum resident'
```

Record: the command, the tree state (`git rev-parse --short HEAD`), the machine
(cores, RAM), and whether the build was warm. A cold-cache number and a warm one
differ by minutes for anything that compiles.

### 3. Change exactly one thing

Two changes in one measurement produce a number that explains neither. When the
second change is tempting, write it down and take it separately.

### 4. Take the number again, same machine, same command

Report the pair. If the delta is inside the run-to-run spread, the honest
conclusion is "no measurable difference" — say that, rather than claiming the
direction that matches the intent.

### 5. Write the number where the code can cite it

The commit message, the doc comment beside the constant, or this table. Not a
chat log, not a scratch file: a measurement nobody can find is a story.

---

## Lock-specific guidance (this is what issue #14 was really about)

Counting lock acquisitions is not the metric. The metric is *contention that
reaches a user-visible wait*, and it has three distinguishable shapes:

| shape | symptom | measurement |
|---|---|---|
| lock held across I/O | a call waits on an unrelated key | a test that holds one key and times a second (`pa-lsp`'s `different_keys_never_wait_on_each_others_acquisition` is the pattern) |
| lock held across a spawn/build | latency scales with the slowest sibling | wall clock of N concurrent calls vs N sequential |
| read-lock coherence traffic | throughput flat as readers grow | readers/sec at 1, 2, 4, 8 threads; flat = contention, rising = fine |

The first two are correctness failures with a latency symptom, and both were
found in this tree without a benchmark: `ensure_client` held a roster-wide write
guard across an LSP handshake, and `pa-mem`'s sharding was argued about before
anything called it. Prefer the test that makes the contention *observable* over
the number that makes it look small.

---

## When NOT to write a perf review

- The change deletes work. Lines removed, allocations removed, a query narrowed:
  the correctness test is the evidence, and a benchmark adds cost without
  information.
- The surface is on the critical path and already has a red-first guard
  (`the_startup_flush_never_blocks_the_first_frame`). Extending that guard beats
  adding a number beside it.
- The code has no caller. See Rule 0.
