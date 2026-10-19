# Independent static review: piped-stdin idle receive policy

## Verdict

**Clean: no blocking findings in the restored/frozen delta.** This is source-only review with existing focused evidence, not a final workspace/release/remote-CI pass.

Reviewed the actual final diff in `crates/pa-cli/src/piped_stdin.rs`, `.research/ci-piped-stdin-recovery.md`, the historical safe receipt, and native policy-red/focused-result summaries. Final source SHA-256 matches the root's frozen hash `8787d25b7a49a86097d416359169d15b5e193f5880b30b009193ae21cb4d1247`. The temporary half-window mutation is gone. AGENTS.md and clean-code guidance were already read. No Cargo/build/test/formatter, source edit, environment/identity change, staging, commit, push, or cleanup was performed by this reviewer. Only this report was created.

## Source and behavior

- `piped_stdin.rs:55-56` adapts the existing real receiver with `|window| receiver.recv_timeout(window)`. The production read is still synchronous std mpsc receive from the same stdin reader thread. It has no new timer, clock, async migration, second collector, test-only production branch, or error translation.
- The existing private `collect_from` at lines 71-84 now accepts `FnMut(Duration) -> Result<Vec<u8>, RecvTimeoutError>` rather than a concrete Receiver. It is the same collector, not a new helper/trait/public API. Its loop is byte-identical to the previous loop after replacing only `receiver.recv_timeout(idle_window)` with `receive(idle_window)`: every iteration passes the same full Duration, bytes append in the same order, Timeout returns `(data, true)`, and Disconnected returns `(data, false)`.
- The restored code has exactly `match receive(idle_window)` with no conditional half-window/reset state. Default 250 ms, cap 30,000 ms, opt-out, TTY handling, stdin reads, UTF-8 decoding/trimming, stderr notice, channel lifetime, and return shape remain unchanged. This is a private runtime refactor for testability, not a byte-unchanged production file; its public behavior is preserved by the direct adapter.
- Ownership remains pa-cli's existing piped-input concern. No new dependency, unsafe code, cross-crate surface, public function, or runtime timeout change was introduced. Readability: **10/10 for this bounded receive-boundary refactor**; no remediation identified.

## Deterministic test contract

`a_live_producer_resets_the_window` at lines 124-145 injects four chunks and then Disconnected through the same collector. It checks the complete 60 ms budget on **every** call, including the initial receive, each post-chunk receive, and the EOF receive. It also checks exact bytes `0123`, `idle_expired=false`, and exactly five calls. Thus a shortened/reused remaining budget after a successful chunk fails at the real loop's receive boundary. It does not claim that scripted chunks consumed wall time or that four logical 20 ms waits occurred.

The existing real-channel EOF test and real open-but-silent timeout test are retained. Independent source comparison confirms each test body is unchanged except the receive-closure adapter; original bytes, EOF/timeout flags, 5,000 ms EOF ceiling, and 30 ms silent-window values remain intact. These tests retain actual std receiver semantics while the deterministic test removes the thread-scheduling premise from the budget-reset oracle. No sleeps, retry-to-green, relaxed flags, or increased timeouts replace the original invariant.

## Actual evidence and limits

- Historical authoritative attempt: pa-cli library **310 passed / 1 failed**, 40.92 seconds, at the old test's `!idle_expired` assertion. Its raw-log hash matches `.research/ci-fullgate-1.98.1-piped-stdin-receipt.json`. The old producer assumed four 20 ms sleeps would be scheduled within repeated 60 ms receiver windows. The exact scheduling delay is unknown; no related production bug is established.
- Native controlled policy-red log shows the intended assertion after a successful first chunk: **left 30 ms / right 60 ms**, 0 passed / 1 failed / 310 filtered, 0.00 seconds, with the temporary half-window mutation. This establishes sensitivity to the receive-budget policy, not that the correct old production implementation was faulty.
- On restored final source, `.research/ci-piped-stdin-filter-green.log` contains **4 passed / 0 failed / 0 ignored / 307 filtered**, 0.03 seconds. This is a focused result. At inspection, the complete CLI-library log had a build-finished line but no completed test-result summary, and focused Clippy output was not present; no completion is inferred from their queued/running state.
- Existing recovery report records Rust 1.98.1 approved DEBUG0/JOBS2 commands; this reviewer did not execute them. Root still owns the full native gate after worker freeze/slot release. No full CLI-library, workspace, release, remote-CI, or new TS-runtime comparison is claimed here. Public behavior is unchanged, so prior parity is not relabeled as a new runtime comparison.

Raw frames/private session/environment data are not reproduced. This reviewer changed no constants or assumptions.
