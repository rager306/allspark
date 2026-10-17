# Reference: RT-005 kernel-error-clip

## Why the task is solvable
Kernel settle clips `ename`/`evalue`/traceback frames at
`DEFAULT_MAX_OUTPUT_CHARS` (65,536 chars) with the marker
`[... output truncated at N chars ...]` (fix `58394f09a`,
`crates/pa-core/src/kernel/manager/execution.rs::clip_at_chars`; the
`kernel_error_clip` suite is the red-first proof). A 1.5MB `ValueError`
therefore lands in the transcript clipped: no 100KB run of `E` exists.

## Oracle rationale
Byte-level STATE on the transcript (marker present, unclipped run
absent), not the prose answer. The clip also protects the provider
request (the error text is re-sent every following turn until
compaction), which is the actual regression the fix closed.
