# CI 39191633430: stopped tty + lifecycle TS comparison

## Environment and scope

Read `AGENTS.md`; edits limited to `crates/pa-cli/tests/termios_process_exit_e2e.rs` pending investigation. No production edits, commits, identity changes, or pushes.

Build environment: `PA_TS_BINARY=/nonexistent/ts CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=2`.

## Confirmed CI observations

- `a_launch_on_a_ctrl_s_stopped_tty_flows_and_exits_whole` timed out waiting for kitty flags push, **not** waiting for tty output. Its captured transcript already contains the full UI and `row 0`, showing the XOFF stop was lifted. The unneeded kitty handshake has a real 250ms response window and is sensitive to CI scheduling.
- Shard 1's lifecycle comparison never ran. `gh release download v0.9.8 --repo "${GITHUB_REPOSITORY}"` failed with `release not found`: the fork repository has no pinned upstream TypeScript release. Download must explicitly target `PrimeIntellect-ai/prime-agent` (reported to parent; workflow outside this task ownership).
- No `crates/pa-cli/tests/lifecycle*.rs` files exist. This is not a lifecycle Rust-test failure or version-branding comparison failure.

## Commands and evidence

1. Existing previously compiled test binary (diagnostic only):
   `target/debug/deps/termios_process_exit_e2e-e38bdb55db1d80f8 a_launch_on_a_ctrl_s_stopped_tty_flows_and_exits_whole --exact --nocapture`
   Exit 0, 1 passed in 0.98s. Log: `.research/ci-tty-existing-binary.log`. This is not retry-to-green acceptance: CI failure is already demonstrated by its recorded transcript; this run only characterizes scheduler dependence.
2. Current-source baseline (exit 0; 1 passed in 0.98s after build):
   `cargo test --locked -p pa-cli --test termios_process_exit_e2e a_launch_on_a_ctrl_s_stopped_tty_flows_and_exits_whole -- --exact --nocapture`
   Log: `.research/ci-tty-red-baseline.log`.

The original TS-reference block is resolved by the verified upstream download below. Full lifecycle parity verifiers have not been run in this task; release fetch and binary identity are proven, not full interactive parity.

## Lifecycle release-source fix and live fetch proof

Changed only the TS-reference download repository in `.github/workflows/ci.yml`: `${GITHUB_REPOSITORY}` → `PrimeIntellect-ai/prime-agent`. Tag `v0.9.8`, asset pattern and checksum are unchanged.

Commands (both exit 0):

```sh
gh release view v0.9.8 --repo PrimeIntellect-ai/prime-agent --json tagName,isPrerelease,assets
gh release download v0.9.8 --repo PrimeIntellect-ai/prime-agent --pattern prime-agent-0.9.8-linux-x64.tar.gz --dir .research/ci-tty-ts-reference
```

Downloaded archive: 60,748,686 bytes. SHA256: `83fb09129bf78e3e60268212cd70932166591b15188caa70c1b0efbcc76235e2` — matches the workflow's pinned checksum. The archive contains root `prime-agent`, `package.json`, runtime and skills; package version is 0.9.8. Metadata: `.research/ci-tty-ts-release.json`; logs: `.research/ci-tty-ts-download.log`, `.research/ci-tty-ts-version.log`. This is the actual TypeScript release, not a Rust replacement. No deploy performed.

## Termios fixture correction

The stopped-launch route is changed test-only: TIOCPKT reports the kernel's actual STOP and START packets, replacing a 300ms absence-of-echo sleep loop; one newline releases the cooked read, rather than CR+LF (two cooked lines). It deliberately does not answer the unrelated 250ms kitty probe. The route still requires rendered `row 0`, clean `/exit`, terminal ledger/termios equality, no leaked slave fd and a flowing cooked echo after exit. Dedicated kitty tests are unchanged. No timeout was increased and no runtime constants were changed.

Focused route passed; raw-lift mutation failed the START oracle as intended; restored full binary passed all 17 tests. See final results below.

Safety note: `pa-cli` deliberately uses its own lint table, not the workspace unsafe-code forbid (documented in its Cargo.toml), because its PTY/process e2e fixtures require OS calls. The packet probe uses three bounded unsafe syscalls with SAFETY comments in the same audited ioctl style as this file's existing fixture, explicitly approved by parent; no Python dependency was added.

## Modified focused route: GREEN

`cargo test --locked -p pa-cli --test termios_process_exit_e2e a_launch_on_a_ctrl_s_stopped_tty_flows_and_exits_whole -- --exact --nocapture`

Exit 0: 1 passed, 16 filtered, route duration 0.78s (shared Cargo queue/build took 11m39s). Log `.research/ci-tty-packet-green.log`. Final fixture is Rust-only, STOP and START packet readiness, three approved bounded unsafe syscalls with SAFETY; no Python dependency, no runtime change, no fixed sleeps in this route.

Completed fixture-only mutation: before the gate marker capture cooked termios, call `enable_raw_mode()` to set its cache, then restore cooked termios through safe nix `tcsetattr` without clearing the cache. The route then arms XOFF normally, but the real mount's cached raw-mode enable is a no-op that cannot clear IXON or lift the stop. Expected oracle failure: kernel output restart/flow readiness. Log `.research/ci-tty-raw-lift-mutation-red.log`.

## Raw-lift mutation: RED, restored

Same focused command with the fixture-only cached-raw/cooked mutation above exited **101**. It failed specifically at the observable START oracle: `waiting for the kernel's output restart` after 30.08s. Thus a real STOP with missing IXON-clearing raw lift is rejected. The mutation was restored exactly as soon as its binary compiled; no mutation remains and parent was notified source-stable. No production source was changed.

Full restored binary command completed exit 0 (17 passed, 2.21s): `cargo test --locked -p pa-cli --test termios_process_exit_e2e -- --nocapture`, log `.research/ci-tty-all-green.log`.

## Full owned termios binary: GREEN

`cargo test --locked -p pa-cli --test termios_process_exit_e2e -- --nocapture` exited 0: **17 passed**, 0 failed, duration 2.21s. Log `.research/ci-tty-all-green.log`. The suspend route reports a loud capability skip when no controlling terminal is present; the three ordinary PTY routes ran. Dedicated kitty tests and shared fixture source remain unchanged.

A diagnostic-only edit then separated poll syscall errors (`rc < 0`) from readiness timeout (`rc == 0`), avoiding misleading `Success (os error 0)` in the red error text. Final exact-source all-termios command also exits **0**, **17 passed** in **2.22s**; log `.research/ci-tty-all-final-green.log`. `rustfmt --edition 2021 --config skip_children=true --check crates/pa-cli/tests/termios_process_exit_e2e.rs` exits 0.

No changed production constants. The STOP/START readiness bound is 30s, matching the shared harness's existing 30s observable waits; the removed 300ms negative-observation delay was not increased. Local generated TS archive/package are excluded in `.git/info/exclude`; only text evidence is intended for review. Process-argument snapshot is not evidence and must not be committed.

## Actual TS/Rust model lifecycle parity: PASS

Ran **`review_model_facts_parity.py`**, exit **0**, `verified=true`, against the verified TS v0.9.8 binary and existing `target/debug/prime-agent` (no rebuild). Sanitized receipt: `.research/ci-model-facts-parity.json`; summary log: `.research/ci-model-facts-parity.log`. The original verifier logic ran through a native `python3` wrapper that only redirected its hardcoded temporary directory into ignored `.research/ci-parity-runtime` and sanitized the receipt before disk. Runtime homes were isolated and auto-removed; providers were loopback-only. No live internet/model calls or ambient settings/auth changes.

Observed identical results across all five actual daemon phases:

1. Initial text model;
2. Explicit text → vision switch;
3. Explicit vision → text switch;
4. Retryable primary HTTP 503 → backup provider;
5. Automatic primary restoration.

Both kernels preserved counters **1,2,3,4,5**. `model.info` and `compact.status.context_window` matched in all phases; complete `compact.status` objects also matched in all five observed probes (not a general token-estimator parity claim). Each binary made **11** loopback provider calls (**9** primary, **2** backup) and persisted exactly one backup-to-primary restoration after the explicit switches. No difference found in this scope.

Binary SHA256: TS `94e90d59b5853036371c6e6a5eb7f5ef3f49d72aa66998be0b41b9d925b1a1c0`; Rust `cc57a09d810c798effe19888969035b88c184b10123363b7d44e445cfe5f1a5c`. Receipt retains only hashes/counts and non-sensitive model probe facts, not prompts, auth, provider request bodies, daemon frames or runtime logs.

`review_draft_disconnect_parity.py` was inspected but not run: its CLI explicitly refuses repository-local receipts, contrary to this task's evidence location requirement. No need to bypass that policy after the relevant lifecycle verifier passed. This report does not claim broad parity across every lifecycle verifier.
