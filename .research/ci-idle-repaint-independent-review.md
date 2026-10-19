# Independent static review: idle-session repaint fixture readiness

## Verdict

**Clean: no blocking findings in the reviewed test-only delta.** This review does not certify the full interactive suite, workspace, release, or remote CI.

Reviewed the actual `crates/pa-cli/tests/interactive_daemon_e2e.rs` diff, the single changed test at lines 4983-5059, `.research/ci-idle-repaint-recovery.md`, the completed final-source native result summary, and the existing headless queue/render behavior needed to assess the oracle. AGENTS.md and clean-code guidance were already read. No Cargo/build/test/formatter, source edit, staging, commit, push, environment/identity change, or cleanup was performed. Only this report was created. Other workers' rendering files were read only, not edited or certified as a new change.

## Ordering and no-input oracle

- The diff is confined to `tui_idle_session_event_repaints_without_input`. The fixture creation, exact second-client Rename request and close, existing renamed-label assertion, and supervisor lifetime are unchanged. No production source, API, dependency, timeout constant, or unsafe code is changed by this fixture patch.
- `interactive_daemon_e2e.rs:5009-5018` first waits for the settled fixture reply to be in a captured attached frame, then sends the existing Timestamp observation, then waits for the renamed label. The external second client waits on the bounded readiness receiver before issuing Rename (`5029-5045`). The initial attachment paint cannot contain this later rename.
- The existing headless driver preserves step order (`crates/pa-tui/src/interactive/render.rs:218-306`). The WaitRender branch at `interactive/run.rs:940-999` only observes captured frames and arms a deadline; it does not set dirty. Timestamp sends its observation and continues before generic input dirtying (`1000-1009`). No Key, Submit, paste, resize, or mouse step is introduced.
- The final HeadlessDone queue entry cannot pass the renamed-label WaitRender before that wait succeeds or expires. Thus its generic input dirtying cannot paint the rename and satisfy the pending wait. Normal headless paint still requires `session.dirty` (`interactive/run.rs:2012-2042`). Renderer finish returns previously captured frames without a final headless repaint (`interactive/render.rs:521`).
- A missing daemon-driven dirty/repaint cannot be repaired by successful WaitRender or Timestamp handling. WaitRender expiry is the important exception: its status note does dirty the UI (`interactive/run.rs:985-995`; `session_ui/notes.rs:90-106`). The new explicit negative assertion rejects that timeout note before accepting the unchanged renamed-label assertion (`interactive_daemon_e2e.rs:5048-5057`). This closes the timeout-induced repaint false-positive route.
- The reused 30-second values are fixture readiness ceilings, replacing guessed 750 ms attachment delay and a four-second TUI lifetime. No product deadline or cadence is relaxed. The run stays alive for the observation it actually needs and remains bounded.

The original no-user-input repaint invariant is therefore retained, not replaced by an input-induced paint. Ownership remains pa-cli's existing real-daemon/TUI integration verifier. Classification: test-only internals. Readability: **10/10 for this bounded change**; no remediation identified.

## Evidence checked

- The historical full interactive receipt is 40 passed / 2 failed in 71.51 seconds, with the idle test missing its renamed label. Its private-log SHA-256 matches the recovery report. The observed frames do not establish whether the old rename ran before attachment or after the old fixed-lifetime run ended. No production defect or exact historical race branch is inferred.
- Final-source `interactive_daemon_e2e.rs` SHA-256 matches the report's frozen hash `695126a014687bfc251ba28bd1fc94017cfdb76c67185c99de7f364fd3992d8f`.
- `.research/ci-idle-repaint-final-green.log` contains the actual native summary: **1 passed, 0 failed, 0 ignored, 41 filtered, 1.95 seconds**; unoptimized test profile. The report records Rust 1.98.1 approved DEBUG0/JOBS2 settings and fmt exit 0. This review did not execute or infer an exit code from the formatter's empty log.
- The earlier timestamp-first build is not used as certification of this final source. No full interactive rerun by this worker or independent mutation of the production dirty flag is claimed.

## Limits

This is source-only review of the fixture change plus existing completed focused evidence. It does not prove arbitrary scheduling, absence of every unrelated repaint source, or the stream worker's separate rendering fix. The ordinary daemon rename handler still supplies its own dirty state (`session_ui/apply.rs:617-621`); this review did not mutate it to run a red-first experiment. Focused green is not a full-gate result. Root retains the authoritative final native gate after all relevant workers freeze/release their slots. Raw frames, secrets, session data, and environment dumps are not reproduced. No constants were changed by this reviewer.
