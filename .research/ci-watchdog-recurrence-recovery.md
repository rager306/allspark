# Recurring watchdog fixture startup failure

Source-only diagnosis. Historical raw log `.research/ci-fullgate-1.98.1-watchdog-recur-red.log`:116864bytes,SHA256 `9ba78072866cb8414096a5d65b51e046d6f472c5258420c82d57acebe06001f1`. Do not commit raw frames.

Factual correction to initial excerpt: diagnostic has2171characters, one kitty capability query, zero flags pushes, alternate-screen enter and attached row0. Child did not stick before tty mount. Failure occurs before `/list` and before either Ctrl+C, so watchdog assertions never execute.

Self-reexec: new session+controlling tty before exec, stdin/stdout/stderr point to its slave, mode/socket env explicit. Each reexec isolates process-global kitty/raw-reader caches; parent harness mutex serializes terminal routes. Mock supervisor uses a bound unique Unix socket and replies create/attach before row0. These startup dependencies succeeded in the historical tape.

Protocol source: raw mode established before reader/probe. Query writes flags+DA query to `/dev/tty`; fixture reply sends flags then DA. Crossterm ordinary EventFilter skips/parks protocol responses, preserving their order. Capability filter consumes flags then flushes DA. Real answer window250ms unchanged. The prior128KiB regression proves buffered reader delay, not the precise cause of this2171-character recurrence.

No source/production edits or Cargo reruns yet. Actual query-observed/answer-write/push timing was not retained in the historical failure; no exact scheduling/reader ownership cause is proven. Next useful evidence is safe monotonic handshake timing and child liveness, not timeout increase, retries or flags removal.

Proposed fixture boundary (not applied): watchdog test uses existing direct-known capability branch via explicit `TERM_PROGRAM=kitty`, keeps real flags-push arming and every restoration/ledger/termios assertion, but does not test ambiguous-terminal RTT negotiation in the teardown route. Dedicated kitty query/verdict tests remain on unknown terminals. `TERM=xterm-kitty` alone is not recognized by current `keyboard_capability`; only listed direct terminal env hints are Supported. Intentional ChildSpec env must be applied after sanitized defaults. Alternative is safe timestamp diagnostics before any fix; no exact recurrence scheduling cause has yet been proved.

## Single approved diagnostics-only run

Explicit native1.98.1/debug0/jobs2/PA_TSnonexistent exact watchdog test exited0:1passed,29filtered,2.13s. Safe diagnostics: query first observed32834us after parent wait began; answer write completed454us after observation; flags push observed8481us after query observation. Child remained alive. Parent observations are NOT actual probe emission timestamps; queued query age and historical reader ownership remain unknown. This pass is diagnostic, not acceptance/retry-to-green. Temporary instrumentation was restored byte-for-byte; no behavior/source change remains. Safe receipt `.research/ci-watchdog-recurrence-diagnostic.json`; raw local log `.research/ci-watchdog-recurrence-diagnostic-local.log` not for commit. Sole Cargo slot released; no further jobs.

## Approved fixture scope correction (source-only, validation pending)

Independent scope review accepted watchdog-only known-direct Kitty capability. This test originally completed negotiation and required a flags push before row0 and before `/list`, so it never tested exit with an in-flight probe. The meaningful invariant is teardown of already armed modes.

Minimal patch: move existing explicit `ChildSpec.env` application after sanitized defaults (before stdio), allowing intentional per-fixture overrides, without inheriting any new ambient variables. Only watchdog route sets `TERM_PROGRAM=kitty`, an exact Supported marker in current production `keyboard_capability`. It waits for the **same exact KITTY_FLAGS_PUSH** then row0; original `/list`,Ctrl+C pair,exit,mode ledger and termios equality are unchanged. Other negotiated routes and dedicated query/verdict tests keep their protocol/deadlines. No helper/API, unsafe, dependencies, retry or timeout change.

This separates teardown testing from ambiguous-terminal discovery; **the historical unknown-terminal negotiation issue is NOT resolved or exonerated**. Prior diagnostic pass does not prove its cause. No production regression/fix is claimed and no artificial mutation is presented as such. Native validation waits for parent's sole Cargo slot.

## Approved-profile validation

All native checks use explicit `PA_TS_BINARY=/nonexistent/ts CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=2` with Rust1.98.1. Exact watchdog run once exits0:1passed,29filtered,2.08s; log `.research/ci-watchdog-direct-focused.log`. This validates scoped teardown fixture, not resolution of historical unknown-terminal negotiation. Fullterminal and dedicated kitty tests run serially next; no retries.

Validation results (all serial, approved explicit profile): fullterminal **30passed13.26s**; dedicated `kitty_verdict_time_e2e` **8passed2.73s**, `kitty_early_typing_e2e` **3passed1.32s**, `kitty_exit_routes_e2e` **2passed20.55s**. Existing dedicated negotiation assertions unchanged. Rustfmt check0. Safe hashes/counts receipt `.research/ci-watchdog-direct-validation.json`; local logs retain raw terminal evidence and are not for commit. Last all-target pa-cli Clippy running.

Final `cargo +1.98.1 clippy --locked -p pa-cli --all-targets -- -D warnings` exits **0** under explicit approved profile. Safe receipt updated with Clippy/fmt outcomes and source SHA256. Source frozen, no Cargo jobs remain; sole slot released to parent. Unknown-terminal historical negotiation cause remains open, not claimed fixed.
