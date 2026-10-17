# Reference: RT-010 compaction-image-heavy

## Invariant
On a history carrying ~21k estimate-tokens of user images (four
2000x2000 PNGs, geometry estimate w*h/750 each) plus growth, the session
must produce a `compaction` transcript row BEFORE any fatal
context-overflow failure, and the run must stay inside its wall-clock
budget.

## Why the task is solvable - and the lane finding (2026-10-17)
* The image-geometry estimate itself is pinned in-repo by the unit
  tests of fix `07364009a` (`user_images_estimate_by_geometry_not_zero`,
  `...elision_marker`, `...shift_the_context_estimate`).
* The behavioral outcome (compaction row before overflow) needs a
  compaction DRIVER. Empirically, the daemonless `-p` print path has
  NONE: a scripted session was driven past 175% of a 20k window (35k
  provider-reported input tokens, `compaction.enabled: true` +
  explicit `keepRecentTokens`/`reserveTokens` in settings.json) with
  zero `compaction` rows and zero overflow handling. The threshold
  supervision lives daemon-side (`pa-daemon/src/compaction_supervision.rs`),
  so this task's lanes are `real-model` (real overflow errors trigger
  the reactive recovery path) and `daemon` (proactive supervision once
  the ACP driver exists - see README "Missing seams").
* The faux print lane is deliberately NOT offered: the faux provider
  does not enforce its declared context window, so no overflow ever
  fires there (its usage estimator also counts images flat, not by
  geometry). A faux-lane variant of this task would be unfalsifiable.

## Oracle rationale
Row-type invariant (`compaction` rows >= 1) + duration bound; no prose
matching. The print-path finding above is itself a deviation-suite
result: headless `-p` sessions have no proactive compaction guard.
