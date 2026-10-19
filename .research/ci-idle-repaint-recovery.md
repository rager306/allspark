# Idle-session repaint fixture readiness

Scope: only `tui_idle_session_event_repaints_without_input` in `crates/pa-cli/tests/interactive_daemon_e2e.rs`. No production/API changes. Stream-render work is independently owned.

## Historical RED
Root full native Rust 1.98.1 interactive gate: **40 passed / 2 failed**, 71.51 s. This idle test failed its existing renamed-label frame assertion. Private frames contained settled fixture reply, no renamed label, no generic error or timeout. Historical log `.research/ci-fullgate-1.98.1-interactive-recur-red.log` is local-only; SHA-256 `386bf4bdf731d0992cdda778990607e770c8a56ce7f745a127d295fa31d3872c`. No raw frames copied here.

Old fixture guessed attachment readiness using external 750 ms sleep and limited TUI life with `WaitMs(4000)`. Under scheduling contention, rename can happen before attachment or after run exit; historical receipt does not distinguish which race branch happened. No independent production mutation claimed.

## Final test-only change
Existing headless steps now wait for settled fixture text, signal existing `Timestamp` readiness, then wait for renamed label. The second wire client waits for that bounded readiness signal before issuing the exact same Rename. No Key/Submit/resize reaches TUI. Existing label assertion unchanged. Added a no-render-timeout-note assertion: a timeout note itself dirties the UI, so it must not accidentally paint a rename and satisfy the label assertion.

Source verification: `interactive/run.rs` WaitRender branch observes captured frames and does not set dirty. Timestamp is handled before generic input dirtying and continues without paint. Headless paint is guarded by `if session.dirty`. Initial fixture wait occurs BEFORE Rename and cannot paint the rename. Thus final wait does not mask a missing daemon-driven repaint except through timeout, explicitly rejected by the new assertion. Existing standard 30000 ms readiness ceilings reused; no production timeout changed.

## Commands
All native tests explicitly use `PA_TS_BINARY=/nonexistent/ts CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=2` Rust1.98.1.
- Initial timestamp-first exact command started before parent's clarification requiring initial fixture WaitRender. Source was corrected while that build was active; its result is not claimed as final-source certification. `.research/ci-idle-repaint-exact-green.log`.
- Final-source exact+fmt command completed once after the first settled: **exit 0**, exact **1 passed / 0 failed** in **1.95 s**, fmt **exit 0**. Logs `.research/ci-idle-repaint-final-green.log` and `.research/ci-idle-repaint-final-fmt.log`. No full interactive run by this worker.
- Final `interactive_daemon_e2e.rs` SHA-256 at source freeze: `695126a014687bfc251ba28bd1fc94017cfdb76c67185c99de7f364fd3992d8f`.

No full-workspace-green claim. Root owns final full gate and independent source review. No cleanup/staging/commit.
