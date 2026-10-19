# Team-picker test readiness recovery

Source stable: only `crates/pa-cli/tests/clipboard_auth_commands_e2e.rs`, failing team-picker test. No production, API, dependency, auth/settings, or other test changes.

## Cause and original RED
Root authoritative Rust 1.98.1 full gate: clipboard_auth_commands_e2e **12 passed / 1 failed**, team-picker existing final Acme status assertion (line 915 before patch). Private historical frames show a successful **personal-account** outcome instead of Acme, not a timeout. The test's fixed 400 ms pacing let Down run before the async team picker was ready; Enter then chose default Personal.

Raw historical log is local-only: `.research/ci-fullgate-1.98.1-team-picker-red.log`. Do not commit raw frames. SHA-256: `296b8d13dcb6d060f9af58b16cd805ce8444cea40a03c378901a5ff4be8f1780`. No frame contents copied to this report.

## Minimal fix
Replaced exactly two `WaitMs(400)` steps with existing `HeadlessStep::WaitRender`: team-picker title before Down, unchanged expected final Acme status after Enter. All assertions remain unchanged. The existing neighboring 30000 ms readiness ceiling is reused; the old 400 ms was pacing, not a product deadline. No product timeout/constant changed.

Historical actual RED is regression evidence. No independent deterministic inverse was attempted: a zero-delay variant could race again, while Notify/Timestamp fixture machinery would over-engineer this two-step fix.

## Commands and outcomes
Environment for tests: `PA_TS_BINARY=/nonexistent/ts CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=2` explicitly on each command.

- `cargo +1.98.1 fmt --all --check > .research/ci-team-picker-fmt.log 2>&1`: exit **0**.
- `PA_TS_BINARY=/nonexistent/ts CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=2 cargo +1.98.1 test -p pa-cli --test clipboard_auth_commands_e2e tui_prime_login_renders_the_inline_team_picker -- --exact > .research/ci-team-picker-exact-green.log 2>&1`: exit **0**, **1 passed**, **0 failed** (one run).
- Then entire `cargo +1.98.1 test -p pa-cli --test clipboard_auth_commands_e2e > .research/ci-team-picker-binary-green.log 2>&1` with same explicit environment: exit **0**, **13 passed**, **0 failed** (one run).

No retry-to-green. Root retains final workspace-gate ownership after explicit slot release.
