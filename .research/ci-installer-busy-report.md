# Installer rollback ETXTBSY fixture diagnosis

Historical final-gate log: `.research/ci-fullgate-1.98.1-installer-busy-red.log`,97115bytes,SHA256 `b939bd86e16e5bdb2035171a90f724a01cbe85d419b20a853cd0b0a7d60168c2`. Only sanitized observations included here.

Failure is FIRST exec of freshly copied `<sandbox>/prefix/.local/share/prime-agent/prime-agent`, at `update_rollback_reads_the_unsuffixed_installer_slot` Command.output: errno26 ExecutableFileBusy. Installer/update production code has not run yet. The six test-side `std::fs::copy` calls run in a multithreaded libtest process alongside sibling test forks/spawns. Unique PID+atomiccounter sandbox paths exclude a same-path test collision.

A copy return closes the parent's writable FD but does not prove a sibling fork has closed its inherited FD before exec. CLOEXEC is close-at-exec, not close-at-fork. Merely sync/drop/atomic-rename preserves the busy inode and cannot solve this ownership race.

Deterministic native OS proof (no unsafe Rust, sleeps or retries): `python3 .research/ci-installer-fd-proof/fd_proof.py`, exit0. Pipe barriers establish child inherited writer before parent closes its writer. Executing the original returns errno26. A waited external `cp` creates a separate executable inode that runs while the inherited old writer remains open. Releasing/reaping the child allows original exec. Safe result `.research/ci-installer-fd-proof.json`:

```json
{"inherited_writer_errno":26,"parent_writer_closed":true,"isolated_copy_exec":true,"after_child_release_exec":true}
```

Scope/limitation: this proves Linux FD inheritance mechanism and isolated-copy boundary, not direct observation of the transient writer during the historical failure. The existing log cannot timestamp FD owners after failure. No production installer issue is proven and no production changes are proposed.

Proposed minimal test correction: one multi-use `copy_installed_build` helper executes external `cp` and waits for success, replacing all six fixture fs::copy sites. Only the one-shot child owns the destination writer; the multithreaded test parent never does. Existing Unix e2e already requires sh/tar; `cp` is a standard Unix fixture command. Rollback and session-preservation assertions remain unchanged. No retries, sleeps, timeout changes, new crates or unsafe.

Approved correction applied only `crates/pa-cli/tests/installer_update_e2e.rs`: multi-use helper waits for external cp success, all six fixture-build copies replaced, every rollback/session/version assertion unchanged. No new permanent race hook, crates or unsafe.

Focused actual failing test GREEN: `env PA_TS_BINARY=/nonexistent/ts CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=2 cargo +1.98.1 test --locked -p pa-cli --test installer_update_e2e -- --exact update_rollback_reads_the_unsuffixed_installer_slot --nocapture` exits0:1passed,8filtered,1.75s. Profile log unoptimized (no debuginfo). Log `.research/ci-installer-busy-focused-green.log`. Full9 running once next.

Full installer binary GREEN: same explicit env+native1.98.1, `cargo test --locked -p pa-cli --test installer_update_e2e -- --nocapture`, exit0, **9passed**,3.76s; log `.research/ci-installer-busy-all-green.log`. Rustfmt check exits0. Final focused Clippy at -Dwarnings running. No retry-to-green or source changes between these runs.

Final focused Clippy with explicit approved env: `cargo +1.98.1 clippy --locked -p pa-cli --test installer_update_e2e -- -D warnings` exits **0**. Log `.research/ci-installer-busy-clippy.log`. Source frozen, no Cargo jobs remain, slot released to parent for final gate.
