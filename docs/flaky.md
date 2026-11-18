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
| `pa-daemon` `kill_cancels_a_mid_provider_wait_turn_and_surfaces_the_aborted_row` | 10-second admission deadline and a 15-second kill budget against a 60-second held response; passed alone (31.8 s) and failed in a later full `--lib` run | fork | 2026-11-18 |

Measured 2026-11-18 on this tree, `CARGO_BUILD_JOBS=2`: two consecutive full
`pa-daemon --lib` runs each failed exactly one unit, and the two units differed
(`1377 passed; 1 failed` both times). `pa-core --lib` was 1843/1843 in the same
window. Both units are in the abort/kill timing class above and are green in
isolation, so the classification is calibration rather than a regression; the
change under test touched neither `worker/` nor the abort path.
