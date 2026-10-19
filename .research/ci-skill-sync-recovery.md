# Skill-sync fixture process boundary recovery

Source frozen: only `crates/pa-core/src/kernel/bootstrap/venv/tests.rs`, `fake_uv` helper plus small helper execution contract test. No production, API, dependency, runtime configuration, timeout, retry, identity, or auth changes.

## Historical RED and limitation
Root authoritative Rust 1.98.1 full gate: pa-core lib **1448 passed / 1 failed / 3 ignored**, 27.55 s. Existing `skill_sync_falls_back_to_per_skill_installs_on_batch_failure` assertion expected **3** logged commands (failed batch plus two per-skill calls), saw **2**, both singles. Assertion and fallback expectations unchanged.

Historical full raw log is local-only `.research/ci-fullgate-1.98.1-skill-sync-red.log`, SHA-256 `1cea09c31fc02064bd2fe7ac83e328e50719418c7ad7f7488cdd47d5ec55744b`. No raw frames copied here.

This proves the batch executable did not log an invocation. The exact historical batch errno is **UNKNOWN** because production intentionally falls back on any batch `run_async` error without reporting its cause. ETXTBSY is a supported mechanism, not a proven errno for the historical run.

## Helper-only fix
Old `fake_uv` wrote the executed inode directly in the multithreaded test process. A sibling fork can inherit that writable descriptor temporarily, leaving the executable busy after the parent closes it. Drop/sync/rename alone does not repair this inode lifetime.

The helper now writes a **non-executed source** `uv-script`, then waits for external `cp(source, uv)` to finish, checks its status, and chmods `uv`. Only the copy child opens the executed inode writable. Parent and unrelated sibling forks can inherit a source writer, but never the executed destination writer. This matches the existing installer fixture's process-owned copy boundary. Source is in the existing temporary directory and follows its normal lifetime.

A small permanent helper contract executes an actual published script and checks exact exit7/stdout `fixture-ready`; it is not claimed as deterministic historical fallback-regression proof. Existing skill-sync 3-call and record/warning assertions remain unchanged.

## Native mechanism receipt
One standalone mechanism proof was executed before the parent's later no-extra-proof message; no repeats. `.research/ci-skill-sync-inode-proof.py` and `.research/ci-skill-sync-inode-proof.log` are safe synthetic fixtures only. Held executable writer yields ETXTBSY; held non-executable source writer plus waited cp yields exact status7/stdout. It proves the inode ownership boundary, not the historical errno.

## Native checks
All commands explicitly use `PA_TS_BINARY=/nonexistent/ts CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=2` and Rust **1.98.1**.
- `rustfmt +1.98.1 --edition 2021 crates/pa-core/src/kernel/bootstrap/venv/tests.rs`: exit **0**.
- `cargo +1.98.1 test -p pa-core --lib kernel::bootstrap::venv::tests::skill_sync_falls_back_to_per_skill_installs_on_batch_failure -- --exact > .research/ci-skill-sync-exact-green.log 2>&1`: **exit 0, 1 passed / 0 failed**, 0.03 s; one run.
- `cargo +1.98.1 test -p pa-core --lib kernel::bootstrap::venv::tests > .research/ci-skill-sync-venv-green.log 2>&1`: **exit 0, 18 passed / 0 failed / 2 ignored**, 0.15 s; one run.
- `cargo +1.98.1 fmt --all --check > .research/ci-skill-sync-fmt.log 2>&1`: **exit 0**, one run.

No worker full-core or full-workspace run. Root retains final full native gate ownership. No workspace-green claim. No cleanup by this worker.
