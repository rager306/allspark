# Piped stdin idle-policy test recovery

## Scope and diagnosis
Only `crates/pa-cli/src/piped_stdin.rs`: existing private `collect_from` accepts a receive closure instead of concrete `Receiver`. The real call remains `receiver.recv_timeout(window)`; loop passes the same fresh full idle window on every iteration. No public surface, dependency, async migration, test-only production branch, or timing constant changes.

Historical root full gate: pa-cli lib **310 passed / 1 failed**, 40.92 s; `a_live_producer_resets_the_window` failed `!idle_expired` at line132. Private full log `.research/ci-fullgate-1.98.1-piped-stdin-red.log` SHA-256 `f34c7c48e5b6aaa2a66477ce22ed5aaa58c676530329d307873527b44ca7228c`. The old test assumed a producer thread sleeping20ms gets scheduled before the60ms receiver timeout. Under load that premise fails while the synchronous production idle policy correctly expires. Exact scheduling delay unknown. Tokio paused time cannot control std::thread::sleep or std::mpsc::recv_timeout; no async rewrite made.

## Deterministic verifier
The live test injects four chunks and then Disconnected, and asserts FULL60ms on EVERY receive, bytes `0123`, idle_expired=false, and5calls. It verifies receive-budget reset policy directly, not actual logical/wall-clock time. No elapsed-time claim. Existing real-channel EOF and silent producer tests retain original byte/EOF/timeout assertions; only invocation adapts to closure. Real silent test retains30ms; no timeout increase.

## Red-first proof
Temporary mutation supplied half the idle window only after first successful chunk (`data` nonempty). Exact native test exit**101**, intended duration assertion **left30ms/right60ms** after first chunk;0passed/1failed,0.00s. Receipt `.research/ci-piped-stdin-policy-red.log`. Mutation removed completely. Final loop again exactly `match receive(idle_window)` with no reset state/bad-toggle left.

## Native commands and pending results
Every command explicitly: `PA_TS_BINARY=/nonexistent/ts CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=2`; Rust **1.98.1**.
- `cargo +1.98.1 test -p pa-cli --lib piped_stdin::tests::a_live_producer_resets_the_window -- --exact`: RED101 as above.
- `rustfmt +1.98.1 --edition 2021 crates/pa-cli/src/piped_stdin.rs`: exit0.
- `cargo +1.98.1 test -p pa-cli --lib piped_stdin::tests`: **exit0, 4 passed / 0 failed**, 0.03s; `.research/ci-piped-stdin-filter-green.log`.
- `cargo +1.98.1 test -p pa-cli --lib`: **exit0, 311 passed / 0 failed**, 43.20s; `.research/ci-piped-stdin-cli-lib-green.log`.
- `cargo +1.98.1 fmt --all --check`: **exit0**; `.research/ci-piped-stdin-fmt.log`.
- `cargo +1.98.1 clippy -p pa-cli --lib -- -D warnings`: **exit0**; `.research/ci-piped-stdin-clippy.log`. Existing external crossterm warnings emitted; no workspace lint failure.

Final source SHA-256 after formatting: `8787d25b7a49a86097d416359169d15b5e193f5880b30b009193ae21cb4d1247`. Source frozen for review. Root owns full workspace gate, no broad green claim by this worker. No cleanup/settings/auth/identity/staging/commit changes.
