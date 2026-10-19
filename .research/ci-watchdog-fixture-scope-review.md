# Independent scope review: watchdog fixture Kitty capability

## Verdict

**The proposed watchdog-only known-Kitty fixture is the correct test-layer boundary, with no scope blocker.** It must retain strict real flags-push and row0 readiness before the unchanged watchdog exercise and retain all exit/ledger/termios assertions. This is approval of that boundary, not review of an unapplied patch, proof of historical cause, or a final-gate pass.

Reviewed current terminal differential harness/main/ledger, `pa-tui` enhanced-key capability and release paths, existing dedicated Kitty verdict/exit coverage, and the sanitized recurrence receipt/report. AGENTS.md and clean-code guidance were already read. No native command, source edit, environment/identity change, staging, commit, push, or cleanup was performed. Only this requested report was created.

## Why this preserves the watchdog contract

The target at `crates/pa-cli/tests/terminal_state_differential_e2e/main.rs:288-314` presently waits for capability query/answer/**actual flags push**, then row0, before sending `/list` into the existing stalled response and the Ctrl+C pair. Its load-bearing behavior is that the watchdog restores a real terminal whose modes were armed. Ambiguous-terminal RTT negotiation is a prerequisite in the current fixture, not the behavior its final oracle measures; it is already completed before the wedge begins.

The real direct-terminal branch at `crates/pa-tui/src/enhanced_keys.rs:290-314` accepts `TERM_PROGRAM=kitty` (or a Kitty window id) only after ruling out transport hints such as tmux/SSH. It emits the same stale-level pops and `ESC[>7u` flags push and sets the same KITTY_ACTIVE state used by the negotiated answer branch at lines 478-494. The same normal release/pop and forced terminal restore apply afterward. No mocked Kitty flags, fake terminal-state success, or product capability override is required.

Accordingly, a watchdog-only fixture can declare known direct Kitty and then wait for exact `KITTY_FLAGS_PUSH` plus attached row0. If Kitty is never enabled, the strict push wait must still fail. `/list` stall, Ctrl+C pair, alt-screen leave, existing exit ceiling/status, full byte-stream ModeLedger, and pre/post termios equality must remain unchanged. Deleting the query expectation for this one route is an explicit removal of unrelated negotiation coupling, not permission to delete the arming proof or weaken restoration.

There is an existing repository precedent, not a newly invented testing pattern: `crates/pa-cli/tests/kitty_exit_routes_e2e/main.rs:130-162` uses the known-terminal axis for ForceQuit and other teardown routes, while separate probed/late-answer axes guard negotiation and standdown. This terminal differential adds whole-mode/termios coverage to that separation.

## Concrete implementation constraint

`ChildSpec` explicit env currently applies at `harness.rs:582-584`, **before** defaults and removals at lines 585-595. A proposed `.env("TERM_PROGRAM", "kitty")` placed there today is silently removed later. Merely adding that spec override would not establish the proposed fixture and would leave the old query dependency.

Intentional per-test overrides must take effect after the common sanitized defaults/removals, scoped to the watchdog's explicit spec. Keep the common removal of inherited tmux/SSH/terminal hints; do not restore ambient capability hints or change every terminal route to known Kitty. Existing override callsites are replay/selector route flags, not terminal-capability assumptions. `TERM=xterm-kitty` alone is not a Supported hint in `keyboard_capability` (`enhanced_keys.rs:207-229`); the existing explicit direct hint is required.

This precedence requirement is a concrete implementation issue, not a demand for a new abstraction, test helper, trait, or timeout.

## Negotiation coverage and remaining risk

Coverage that must remain separate and unchanged:

- Other terminal differential routes still call `answer_kitty_query` under sanitized `xterm-256color`, so query/answer/push startup remains exercised outside the watchdog route.
- `kitty_verdict_time_e2e.rs:87-258` has real-query served-path checks, DA1 early conclusion, 60/100 ms late replies, a silent-terminal late upgrade, immediate Kitty control, and raced suspend/deadline assertions. These check actual protocol timing; they are not inferred from environment hints. Their query assertions prevent a silent direct-capability shortcut from masquerading as a negotiation pass.
- `kitty_early_typing_e2e` checks typing through the probe window and Kitty/DA1 outcomes. `kitty_exit_routes_e2e` retains a probed normal exit and late-answer standdown; the release-handoff fixture checks in-flight releases.

The watchdog-only direct path will no longer independently observe ambiguous query/answer startup or residual probe-thread interactions on this teardown route. It therefore cannot diagnose or prove away the recurring negotiation failure. That loss of duplicate startup coupling is acceptable for this test's armed-modes teardown contract, provided the dedicated negotiated tests remain enabled and their real native results are reported separately. A pass here is not a protocol-timing result or evidence that the unknown-terminal branch is healthy.

## Historical evidence and limits

The safe recurring receipt records **29 passed / 1 failed**, 41.26 seconds. The failing watchdog fixture had one query, zero pushes, alt-screen enter, and attached row0. Its raw-log hash matches the sanitized receipt. This means the child mounted and painted, and the failure happened before `/list`, either Ctrl+C, or watchdog restoration assertions. It does not establish late scheduling, a slow OS, reader ownership, a bad reply, or a product negotiation defect. The earlier synthetic 128 KiB buffered-reader regression does not identify the cause of this small recurring trace.

No source patch was reviewed in this scope report and no fresh diagnostic/native pass is claimed. Do not classify this change as a production negotiation fix. It is a test-only choice of the known direct terminal premise; production probe timing, capability policy, and all budgets should remain unchanged. Root owns final implementation review/native evidence/full gate. Privacy-sensitive frames, session content, and environment dumps are not reproduced.

Source/ownership assessment: pa-cli integration fixture only; existing pa-tui capability API/path is reused, with no new public API/dependency/unsafe/runtime constants justified. Readability of the proposed minimal boundary: **10/10 if implemented as the explicit route override plus strict existing readiness/oracles**; this is not a rating of an unseen patch.
