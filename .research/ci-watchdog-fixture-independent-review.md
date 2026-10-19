# Independent review: actual watchdog-only known-Kitty fixture

## Verdict: clean, within the approved test-only scope

No concrete blocker found in the actual two-file diff. This review supersedes no historical diagnosis: it approves deterministic armed-mode teardown setup, **not** a fix for unknown-terminal negotiation. The historical query=1/push=0 cause remains unproved.

- `harness.rs:582-595` retains the same common terminal defaults and inherited-hint removals, then applies explicit `ChildSpec.env`. The watchdog's intentional direct-Kitty hint now survives sanitization. Other specs keep sanitized defaults; the existing selector/replay overrides do not overlap those terminal keys. The entire harness diff is precisely that move and the required command-chain split.
- `main.rs:292-319` changes only the watchdog setup: `TERM_PROGRAM=kitty`, followed by the same real `KITTY_FLAGS_PUSH` readiness. The actual on-wire push remains mandatory; row0 remains mandatory. This does not infer arming from environment alone.
- From the row0 wait onward, `/list`, the stalled-list mock, both Ctrl+C writes, ALT_SCREEN_LEAVE, the 20-second child-exit ceiling, `Some(0)`, and the full ledger/termios restoration oracle are byte-identical to HEAD. No timing constant or budget changed.
- The entire main-file diff is that single setup replacement. Exactly one query/answer call is removed (11 to 10); other negotiated differential routes retain their original setup. Dedicated verdict, early-typing, exit-route and release-handoff tests, ledger, enhanced-key production path, and watchdog production path have empty working-tree diffs in the inspected snapshot.

The existing production direct-Kitty branch and negotiated branch arm the same flags/state; final restore coverage remains meaningful. The prior scope report explains the intentionally separate negotiation coverage and the loss of duplicate query startup coverage in this watchdog route. A new pass here cannot prove that the ambiguous-terminal branch is healthy.

## Review freeze

SHA-256 of the reviewed source bytes; re-read and matched immediately before this report:

| Source | SHA-256 |
|---|---|
| `crates/pa-cli/tests/terminal_state_differential_e2e/harness.rs` | `330e14ff795bb4d8289e5c0e6491074a246aeae42ce37213db7d8e643fbe3436` |
| `crates/pa-cli/tests/terminal_state_differential_e2e/main.rs` | `b81e1e2b8876a9249cfa8efa993d77bed8d972d6582f7a88fc114b9eb0c2e252` |

## Evidence boundary and ownership

Source-only review against HEAD, using read-only git diff/show and file reads. No Cargo/test/format command was run by this reviewer. Native/final-workspace gates remain owned by root and workers; no queued or partial result is certified here. No raw frame, actual environment value, session content, or user data was read or reproduced.

No source modification, public API, helper, dependency, production change, or global environment mutation was introduced by the reviewed diff. Ownership stays within the existing pa-cli integration fixture. The only changed fixture premise is known direct Kitty for this watchdog; unchanged time budgets and strict restoration assertions remain load-bearing. Only this requested review report was written.
