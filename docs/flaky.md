# Flaky and environment-gated units

Every known-flaky or environment-gated test is listed here with its owner and
the date it was last triaged. A `#[ignore]` in the tree without a row here is
the defect this ledger exists to prevent.

| unit | why | owner | triaged |
|---|---|---|---|
| `pa-cli/tests/terminal_state_differential_e2e` (suspend cycle) | needs a controlling terminal that survives the runner's own pty; fails identically on the pre-migration code | fork | 2026-10-26 |
| `pa-cli` pty e2e suite under a nested `script` | nested ptys make the child hang at startup; per-binary runs are green | fork | 2026-10-26 |
| `pa-cli/tests/installer_update_e2e` | needs ~10 GB transient (each sandbox copies the debug binary); run it with the disk swept | fork | 2026-10-26 |
