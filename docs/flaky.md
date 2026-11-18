# Flaky and environment-gated units

Every known-flaky or environment-gated test is listed here with its owner and
the date it was last triaged. A `#[ignore]` in the tree without a row here is
the defect this ledger exists to prevent.

| unit | why | owner | triaged |
|---|---|---|---|
| `pa-cli/tests/terminal_state_differential_e2e` (suspend cycle) | needs a controlling terminal that survives the runner's own pty; fails identically on the pre-migration code | fork | 2026-10-26 |
| `pa-cli` pty e2e suite under a nested `script` | nested ptys make the child hang at startup; per-binary runs are green | fork | 2026-10-26 |
| `pa-cli/tests/installer_update_e2e` | needs ~10 GB transient (each sandbox copies the debug binary); run it with the disk swept | fork | 2026-10-26 |
| `pa-daemon` `abort_in_flight_turn_cancels_a_running_kernel_cell` | fixed 3-minute kernel-boot deadline and a fixed 3-second post-abort sleep, calibrated for an unloaded runner; passes alone and in its own module, failed once in a full `--lib` run at 151% CPU | fork | 2026-11-18 |
| `pa-daemon` `kill_cancels_a_mid_provider_wait_turn_and_surfaces_the_aborted_row` | **not a flake: a fixed defect.** It failed in a full `--lib` run with `kill waited out the held provider response (60.311663675s)` and passed alone (28.65 s, 3/3) including under ten CPU burners. 60.31 s was the held reply, so the abort never cancelled the fetch — the agent dropped an abort that arrived before its run slot was filled. Fixed in `7804fc3a7`; the unit test `an_abort_before_the_run_starts_is_not_lost` now covers the window | fork | 2026-11-18 |
| `pa-daemon` `worker::tests::abort_boundary_tests::*` and `agent_engine::tests::goal::active_goal_aborted_turn_row_*` | **mostly a real defect, partly a budget.** Measured 2026-11-18: three consecutive full `--lib` runs each failed one test of this family (1389/1 every time, a different test each run) and every failed test passed alone. The causes were two: (1) the abort lost its signal when it landed between the worker's busy flag and the agent's run slot — fixed in `1cfaf952e`, with `an_abort_inside_the_start_window_ends_the_run` pinning the window deterministically; (2) four post-abort idle waits used a literal 15 s, while the test alone takes 26.19 s — now `IDLE_AFTER_ABORT_BUDGET` (45 s, 15 s below the 60 s hold) with the derivation at the constant, so raising it further would weaken the assertion rather than fix anything. Full suite 1390/0 since. Note the gate cannot isolate a single test inside `--lib`: units are whole test targets | fork | 2026-11-18 |

Measured 2026-11-18 on this tree, `CARGO_BUILD_JOBS=2`: two consecutive full
`pa-daemon --lib` runs each failed exactly one unit, and the two units differed
(`1377 passed; 1 failed` both times). `pa-core --lib` was 1843/1843 in the same
window. Both units are in the abort/kill timing class above and are green in
isolation, so the classification is calibration rather than a regression; the
change under test touched neither `worker/` nor the abort path.

Measured 2026-11-18, the same tree, `CARGO_BUILD_JOBS=2`:

| run | competing work | result |
|---|---|---|
| 1 | `pa-core --lib` building and testing alongside | 1377 passed, 3 failed (abort-boundary, goal-abort, kernel-cell abort) |
| 2 | none | 1380 passed, 0 failed |

The classification is load, not a regression: every failing unit passed in
isolation, and the tree changed between the runs only by commits that touch the
project store and the jev settings gate. No abort or queue code changed.
