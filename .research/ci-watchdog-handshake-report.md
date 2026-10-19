# Terminal-state watchdog handshake failure

Historical full-gate log SHA256: `234f5a817fb6d26695956cd80d8c1999e5974f6b1ccb60060ac0fd84fadf9b47` (119820 bytes). Raw terminal frames remain local and are not included here. Failure occurred before watchdog oracle: one kitty query, zero flags pushes, row0 rendered.

The shared mock terminal reader sleeps 20ms unconditionally after each read of at most 8192 bytes, including after reading a complete query. A buffered first paint therefore consumes the real 250ms capability-probe response budget without waiting for any new data.

Regression added first: `harness::a_buffered_terminal_query_is_answered_within_the_probe_window` uses a UnixStream mock terminal, buffers128KiB of paint before the query and enforces the real250ms reply deadline. It preserves the actual kitty reply bytes. No production changes, timeout increases, retries or relaxed flags assertions.

Red command: `cargo +1.98.1 test --locked -p pa-cli --test terminal_state_differential_e2e a_buffered_terminal_query_is_answered_within_the_probe_window -- --exact harness::a_buffered_terminal_query_is_answered_within_the_probe_window --nocapture`. Log: `.research/ci-watchdog-buffered-query-red.log`.

Limitation: historical failure diagnostic is only2171 characters, not128KiB. The synthetic test proves a real buffered-reader latency defect at the unchanged250ms protocol deadline; it does not claim the historical watchdog failure had the same prelude size. Historical scheduling delay is not directly timestamped. No production kitty timing defect is inferred from missing flags alone.

Red result: **exit101**, expected deadline assertion failed in0.41s with old reader; no retry was used. Fix then applied: safe `nix::poll` waits for descriptor readiness and checks newly read bytes immediately on loop re-entry, removing unconditional20ms sleeps. Existing pa-cli DEV nix0.29 dependency gained only `poll` feature; no new crate, no new unsafe, no production dependency/API or probe deadline changed. Native Cargo.lock resolution is verified below; its bytes did not change.

Native `cargo +1.98.1 metadata --offline --format-version1` resolved feature change successfully; Cargo.lock unchanged byte-for-byte SHA256 `e5b10843183226e49a04fffe4c037083ae40232a8f482856fde1666adab3b7fc`. Existing DEV nix feature `poll` is needed explicitly so focused pa-cli tests do not depend on pa-perf workspace feature unification.

Green command (clean single filter): `cargo +1.98.1 test --locked -p pa-cli --test terminal_state_differential_e2e -- --exact harness::a_buffered_terminal_query_is_answered_within_the_probe_window --nocapture`; log `.research/ci-watchdog-buffered-query-green.log`.

Focused regression GREEN: **exit0**,1 passed,29filtered,0.06s (native1.98.1 build3m30s). Safe poll resolves without relying on workspace feature unification. The single full30 result below preserves every existing route and kitty flags/ledger oracle.

Full owned terminal differential GREEN: `cargo +1.98.1 test --locked -p pa-cli --test terminal_state_differential_e2e -- --nocapture` exits0: **30 passed**,0failed,13.30s. `the_force_quit_watchdog_restores_every_mode` explicitly passes. Log `.research/ci-watchdog-terminal-all-green.log` is local terminal evidence, not intended for commit. No route/flags/ledger assertion was removed or weakened. The focused native1.98.1 Clippy result is recorded below.

Profile caveat: a Python-kernel restart dropped the four orchestration env vars before green/full30/Clippy commands. Green log explicitly says unoptimized+debuginfo. The observed results above are real, but not the approved debug0 profile. No rerun-to-green is performed; parent finalnative gate with `PA_TS_BINARY=/nonexistent/ts CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=2` is authoritative. Future commands explicitly restore these four constants.

Final focused Clippy: `cargo +1.98.1 clippy --locked -p pa-cli --test terminal_state_differential_e2e -- -D warnings` exits **0** (log `.research/ci-watchdog-focused-clippy.log`). Source frozen, no Cargo jobs remain, sole slot released to parent for authoritative approved-profile full gate.
