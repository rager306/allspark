# New issue: `@file` ingestion has no size cap — a single argument embeds a whole file verbatim into the initial prompt

**Status:** draft / not submitted. **Type:** bug (unbounded prompt / cost). **Severity:** P2. **Area:** `crates/pa-cli/src/file_processor.rs` (port of `packages/coding-agent/src/cli/file-processor.ts`).

## Summary

The `@file` text path reads the file **in full** and embeds it verbatim inside a `<file name="…">` block, with no byte cap and no truncation marker. The image path has a 4.5 MB gate (`auto_resize_images`), but the text path is unbounded — one `@file huge.log` pulls an arbitrarily large payload into the first message. This is a faithful port of the TS v0.9.8 behaviour, so it is a deliberate divergence to fix.

## Reproduction (CLAUDE.md-style)

```
$ head -c 200000000 /dev/urandom | base64 > huge.txt   # ~200 MB single file
$ prime-agent "@huge.txt explain this"                 # (or claude-style @file arg)
```

The whole file is read (`std::fs::read`) and written verbatim into `processed.text`; the request carries ~200 MB into the initial prompt (billed, and often rejected by the provider or forcing an immediate compaction).

## Evidence (both eras)

| Era | File:line | Code |
|---|---|---|
| TS `v0.9.8` | `packages/coding-agent/src/cli/file-processor.ts:76-77` | `const content = await readFile(absolutePath, "utf-8"); text += \`<file name="${absolutePath}">\n${content}\n</file>\n\`;` — **no cap**, whole file, verbatim |
| TS `v0.9.8` | `…/file-processor.ts:41-73` | images get a resize/omit gate (`resizeImage`, "could not be resized below the inline image size limit"); text has **none** |
| Rust `v0.10.x` / `main` | `crates/pa-cli/src/file_processor.rs:68` | `let bytes = std::fs::read(&absolute_path)` — reads the entire file |
| Rust | `file_processor.rs:71-75` | `String::from_utf8_lossy(&bytes)` embedded verbatim in `<file name=…>`, no marker |
| Rust | `file_processor.rs:52` (`DEFAULT_MAX_BYTES = 4_500_000`) | the only size gate, and it applies **only** to images and only when `auto_resize_images=true` |
| Rust | `file_processor.rs:193-202` | with `auto_resize=false`, even >4 MB image payloads are attached raw |
| Rust | `crates/pa-cli/src/lib.rs:348` | caller: the expanded text goes into the first message; `initial_message.rs` adds no cap |

Module doc-comment `file_processor.rs:3-8` explicitly states the text path mirrors the Node/TS read (lossy decode + verbatim embed), so a cap is a documented parity divergence.

## Proposed fix

Cap the text embed at a byte budget, read only up to the cap, and append an explicit truncation marker; cut back to a UTF-8 char boundary so the marker never follows a split character.

Key fragment (`file_processor.rs`):

```rust
const MAX_TEXT_EMBED_BYTES: u64 = 256 * 1024;   // ~5x the 50KiB tool-output byte cap, small vs the 4.5MB image limit

let total_bytes = metadata.len();
let mut file = std::fs::File::open(&absolute_path)?;
let mut bytes = Vec::new();
(&mut file).take(total_bytes.min(MAX_TEXT_EMBED_BYTES)).read_to_end(&mut bytes)?;  // bounded read
let mut embedded_bytes = bytes.len();
if total_bytes > MAX_TEXT_EMBED_BYTES {
    if let Err(e) = std::str::from_utf8(&bytes) { if e.error_len().is_none() { embedded_bytes = e.valid_up_to(); } }
}
bytes.truncate(embedded_bytes);
let mut content = String::from_utf8_lossy(&bytes).into_owned();
if total_bytes > MAX_TEXT_EMBED_BYTES {
    let _ = write!(content, "\n[file truncated at {embedded_bytes} of {total_bytes} bytes]");
}
```

Tests added: oversized file → prefix + marker; exactly-at-cap → full; single giant line; multibyte char straddling the cap is never split.

## Impact

- A single `@file` argument can pull an unbounded payload into the first prompt → provider rejection, massive token cost, or an immediate forced compaction.
- Trigger is a user-supplied CLI argument (self-inflicted, no remote vector), hence P2 rather than P1.
- Also feeds the session-resume replay path (a multi-MB `@file` embed lands as the first JSONL row).

## References

- `.research/verify-unbounded.md` #4; `.research/verify-ts-era.md` §C (#4).
- TS reference: `git show v0.9.8:packages/coding-agent/src/cli/file-processor.ts`.
