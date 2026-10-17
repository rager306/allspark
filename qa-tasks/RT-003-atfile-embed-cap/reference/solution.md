# Reference: RT-003 atfile-embed-cap

## Why the task is solvable
`pa-cli/src/file_processor.rs` caps every text `@file` at
`MAX_TEXT_EMBED_BYTES` (256KiB): larger files embed the prefix plus
`[file truncated at N of M bytes]` (fix `5461ba80a`, tests
`oversized_text_files_embed_the_prefix_and_a_truncation_marker` and
`a_single_giant_line_is_truncated_like_any_other_file`). The read itself
stops at the cap, so neither the prompt nor the session's first message
carries the multi-MB payload.

## Oracle rationale
The oracle looks at STATE (the initial user message in the session
JSONL): bounded block size, marker present, tail absent. The faux lane
makes it deterministic in CI; with a real model the same oracle holds
(the harness owns the embedding, not the model).
