# Independent static review: frame-local streaming entry rows

## Verdict

**Clean: no blocking findings in the reviewed production/test delta.** This is independent source review with existing native evidence, not final-gate certification.

Reviewed the actual diff in `crates/pa-tui/src/view/lazy.rs` and `lazy_tests.rs`, row ownership/cacheability, frame and selection composition, existing bounded-history/selection tests, `.research/ci-stream-recurrence-report.md`, and result summaries from the counter-red and complete pa-tui runs. AGENTS.md and clean-code guidance were already read. No Cargo/build/test/formatter, source edit, environment/identity change, staging, commit, push, or cleanup was performed. Only this report was created.

## Source findings

- `lazy.rs:427` replaces an old write-only `touched` vector with a local `HashMap`; `lazy.rs:476-479` fetches each ordinary entry once per `visible_transcript_window` invocation and clones its `EntryRows` for later backward-walk, forward-fill, or boundary-peek use. Reversing these exact substitutions reproduces the whole prior production file byte-for-byte. No other runtime change is present in this delta.
- The map is invocation-local, not an AgentView field or cross-frame cache. Width and detail are set before it is created; the synchronous walk does not await, dispatch input, mutate chat content, or change entry presentation between lookups. Cache/click/window bookkeeping is mutable, but logical entry content remains fixed within the call. A fallback recursive window build creates a separate local map. Future streaming updates and stale/width/detail invalidation still use the existing paths.
- `layout.rs:147-175` defines both EntryRows variants as Arc-backed values. Clone shares the immutable row storage rather than cloning all Lines. `range()` returns owned/cloned output rows; selection restyling later changes those output rows, not the shared map entries. No aliasing or selection-style leakage was found.
- The map is only keyed/queried, never iterated for output. It cannot reorder sections. Cursor traversal, anchor/movement math, click section recording, bottom detection, splash/tail laziness, tail geometry delta, following/paused state, and selection coordinates remain the same source as before. `frame.rs:183-187` still applies selection styling after the visible rows are returned. The separate selection-extraction walk is unchanged.
- The patch does not introduce an offscreen/full-history traversal. It retains entries the existing walk actually reaches until this invocation ends; that is not a strict viewport-only memory bound for a large jump. Packed stable entries add Arc references, while fresh transient rows survive long enough to avoid duplicate rendering in the same walk. Entries reached only for movement/peek may also be retained during the call. This bounded lifetime is not a new persistent cache or a claim that arbitrary history jumps become constant work.
- The old vector had only declaration/push sites and no read or invalidation role. Its removal does not discard an existing cache-eviction responsibility. Existing `sparse_entries`, persistent packed layouts, and mutation-height bookkeeping remain untouched.
- Ownership is pa-tui private rendering internals. No new public API, dependency, crate/re-export, unsafe code, render cadence, production timeout/budget, or E2E assertion change was introduced. Readability: **10/10 for this bounded implementation**; no remediation identified.

## Test validity and actual evidence

The additive unit `lazy_tests.rs:98-133` drives a real streaming assistant. It resets the existing thread-local ENTRY_RENDERS counter immediately before sparse composition, requires exactly one entry render, then compares the complete styled frame to a separate dense view. Before each dense comparison it sets both `sparse_enabled=false` and `sparse_window=None`; that reference uses full layout/transcript materialization rather than this new sparse map. Equality is on complete Line/Span values, not plain text alone.

The empty suffix exercises another unchanged-content streaming frame; later appends introduce bold Markdown and Unicode. Both views use existing mutation preparation/stale marking. Comparing the next frames to the dense result guards against stale cross-frame reuse. The test does not itself claim paused/selection coverage; existing tests provide those additional scenarios.

- Actual native counter-red log: the new test fails with **left 2 / right 1**, 0 passed / 1 failed, 1,491 filtered, 0.05 seconds, before the runtime substitution. This observes duplicate real entry renders, not a synthetic timer or test-only production branch.
- Actual native `.research/ci-stream-frame-reuse-pa-tui-green.log`: **1,492 passed / 0 failed / 0 ignored**, 31.61 seconds, including the new count/full-styled-frame/fresh-update unit.
- The same full-pa-tui log records existing dense/frame/selection and 100k-entry bounded-history tests passing, including paused detail toggles, offscreen growth, same-window copy bounds, growing-stream selection, unseen dynamic mutation, and top/tail selection. Source retains their original traversal/row-count assertions.
- The producer E2E log had no completed result summary when inspected; no focused producer or concurrent-suite success is inferred here. The historical recurrence remains evidence that the earlier autolink-only focused green did not establish enough concurrent headroom.

Reviewed source hashes: `lazy.rs` SHA-256 `843732533e98fe32b6e4d3d51c04fc357c96216edcd6968f542aee05307fbdf9`; `lazy_tests.rs` SHA-256 `c7b8f850b2f904d2a0c7f5c4ac8a8eb6aeadeaabf5bb278a17cccaf1d60a8e97`.

## Limits

The deterministic red/green proves duplicate entry work is removed within the sparse invocation while preserving the tested styled output. It does not prove the full producer test, concurrent interactive/workspace gate, release build, remote CI, or every input. The new unit is a dense Rust reference comparison, not a new TS binary parity run; previous autolink TS evidence is not relabeled as post-patch evidence for this delta. Root retains final approved-profile native gate ownership after worker freeze/slot release. No raw frames/private session/environment data are reproduced, and no constants were changed by this reviewer. Idle fixture review was completed separately in `.research/ci-idle-repaint-independent-review.md`.
