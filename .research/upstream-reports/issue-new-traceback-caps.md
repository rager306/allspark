# New issue: Kernel error events (`ename`/`evalue`/`traceback`) bypass the settle cap — megabyte exception reprs are re-billed every turn

**Status:** draft / not submitted. **Type:** bug (unbounded payload / cost). **Severity:** P1. **Area:** `crates/pa-core/src/kernel/manager/events.rs` + `…/manager/execution.rs` (+ `crates/pa-core/src/tools/ipython.rs`).

## Summary

The kernel settle cap clips `stdout`/`stderr`/`result`/`background`, but **not** the error payload: `Event::Error` moves `ename`/`evalue`/`traceback` into the buffers whole, and settle passes them through untouched (`error: buffers.error.take()`). A megabyte `AssertionError`/`ValueError` repr (e.g. a pandas DataFrame) then rides into the transcript and is re-sent to the provider on every subsequent turn until compaction. The kernel-side cap is `_RESULT_TEXT_CAP = 1 MiB` **per line**, not 64 KiB for the whole error.

## Reproduction

```python
import pandas as pd
big = pd.DataFrame({"a": range(1_000_000)})
assert False, big          # AssertionError with a multi-MB repr in evalue + traceback
```

The tool result (`format_execute_text`) appends `error.traceback.join("\n")` verbatim; a `repr` in the low-MB range passes freely (the 32 MiB protocol cliff is far away). Repeat a turn and the same text is re-billed.

## Evidence (both eras)

| Era | File:line | Code |
|---|---|---|
| TS `v0.9.8` | `packages/coding-agent/src/core/kernel/repl-manager.ts:1210-1213` | settle caps **only** `stdout`/`stderr`/`result` (markers); `:1220-1221` background; `:1231` `error: execution.error` — passed whole |
| TS `v0.9.8` | `packages/coding-agent/src/core/tools/ipython.ts:729` | `text += (text ? "\n" : "") + r.error.traceback.join("\n")` — whole traceback into the tool result |
| Rust `v0.10.x` / `main` | `crates/pa-core/src/kernel/manager/events.rs:71-94` | `Event::Error { … } => buffers.error = Some(KernelError { ename, evalue, traceback })` — no cap at ingest |
| Rust | `crates/pa-core/src/kernel/manager/execution.rs:230` | `error: buffers.error.take()` — settle passes it through (contrast the result clip at `:189-199` and stdout/stderr markers) |
| Rust | `crates/pa-core/src/tools/ipython.rs:251-255` | `format_execute_text` pushes `error.traceback.join("\n")` verbatim into the tool result |
| Rust | `crates/pa-core/src/kernel/shared.rs:218` | contract comment: cap covers `stdout / stderr / result` only (error not listed) |
| Runtime | `prime-agent-runtime/src/rlm/repl.py:53-54, 540-563` | kernel-side `_RESULT_TEXT_CAP = 1_048_576` **per line** for `evalue`/traceback (commit `0e3c90022` "cap error text", 2026-09-22) — a 1 MiB/line cap, not 64 KiB for the whole error |

Downstream there is no second cap: pa-agent tool results are not truncated (only the bash tool truncates). The >32 MiB protocol cliff (`MAX_PROTOCOL_LINE_BYTES = 32 MiB`, `manager/mod.rs:51`) is shared with the Result-frame case.

## Proposed fix

Clip the error payload at settle exactly like `result`, sharing the same marker convention: clip `ename` and `evalue` each, and clip the traceback on its **joined** length (frames kept whole while they fit; the frame that crosses the bound is cut; the rest dropped).

Key fragment (`execution.rs`):

```rust
fn clip_at_chars(mut text: String, max_chars: usize) -> String {
    if let Some((end, _)) = text.char_indices().nth(max_chars) {
        text.truncate(end);
        let _ = write!(text, "\n[... output truncated at {max_chars} chars ...]");
    }
    text
}

let error = buffers.error.take().map(|mut error| {
    error.ename  = clip_at_chars(error.ename,  execution.max_chars);
    error.evalue = clip_at_chars(error.evalue, execution.max_chars);
    let mut remaining = execution.max_chars;
    let mut frames = Vec::new();
    for (i, frame) in error.traceback.into_iter().enumerate() {
        if i > 0 { remaining = remaining.saturating_sub(1); }   // join separator shares the budget
        let n = frame.chars().count();
        if n > remaining {
            let mut cut = frame;
            if let Some((end, _)) = cut.char_indices().nth(remaining) { cut.truncate(end); }
            let _ = write!(cut, "\n[... output truncated at {} chars ...]", execution.max_chars);
            frames.push(cut); break;
        }
        remaining -= n; frames.push(frame);
    }
    error.traceback = frames; error
});
```

Also update the `max_output_chars` doc-comment (`shared.rs:218`) to name the error payload. Test: `crates/pa-core/tests/kernel_error_clip.rs` (megabyte repr clipped at the default cap, small error byte-identical, char-boundary cut, reader keeps serving after the clipped settle). The fix is local hardening: it does not change the wire for normal-size errors.

## Impact

- A megabyte exception repr rides into the transcript and is **re-billed to the provider on every subsequent turn** until compaction — real, compounding cost.
- Plausible trigger: `AssertionError`/`ValueError` carrying a large `repr` (DataFrames, big lists/dicts).
- The `>32 MiB` protocol cliff (`fail_protocol_frame` → kill + respawn + restore) is a shared failure mode.
- Both eras carry the asymmetry (host-side cap omits `error`; kernel cap is 1 MiB/line).

## References

- `.research/verify-unbounded.md` #11; `.research/verify-ts-era.md` §C (#11).
- TS reference: `git show v0.9.8:packages/coding-agent/src/core/kernel/repl-manager.ts` (1206-1235) and `…/tools/ipython.ts` (729).
