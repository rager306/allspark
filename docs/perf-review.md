# Performance review procedure

## What to measure

Before changing anything on a hot path, take a number on the current code.
State the baseline in the commit message. Without a baseline, the change is
a claim, not a result.

## How

1. Identify the scenario (a user-visible action, not a microbenchmark).
2. Run it on a quiet machine; record wall clock.
3. Run it again with `--repeat 5`; record median.
4. Change one thing.
5. Repeat steps 2–3.
6. Report both numbers.

## Where baselines live

`crates/pa-perf/baseline.json` — per-platform, per-scenario.
The gate compares current p99 frame time against the baseline and fails on
a 10% regression (`report.rs` `REGRESSION_FACTOR`).

## What to do when the number regresses

- Under 10%: note it, merge, update the baseline.
- Over 10%: revert, investigate, fix, re-measure.

## Lessons

- A cache that stops noticing edits is a correctness defect, not a performance
  issue (measured: the bundler catalog shipped synthetic models because a
  fallback path silently replaced real data).
- A guard that cannot fire is not a guard (prepush-gate disk floor compared
  an empty string for 275 log lines).
