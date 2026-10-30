---
name: edit
description: Replace an exact, unique string in an existing file. Use for targeted single-occurrence edits to files from the Python kernel instead of rewriting the whole file. Preserves the file's line endings and BOM.
---

# Edit

Make a targeted edit to an existing file by replacing one exact, unique
occurrence of a string. `old_str` must appear exactly once in the file.

Call directly from the kernel:

    await edit(path="pkg/file.py", old_str=old, new_str=new)

Use exact old/new strings. If the text contains triple double quotes, use
triple single-quoted variables (`old = '''...'''`) or build `old`/`new` from
inspected file slices. Returns a short confirmation; raises if `old_str` is
missing or matches more than once (widen the snippet to make it unique).

Or from a shell cell:

    !edit --path pkg/file.py --old-str "..." --new-str "..."

## Contract

- **Line endings are preserved.** A CRLF file stays CRLF, so a one-line edit is
  a one-line diff, not a whole-file rewrite. An `old_str` written with LF still
  matches a CRLF file: both sides are normalized for comparison and the file's
  own ending is restored on write.
- **A BOM survives.** The file's leading byte-order mark is written back.
- **Non-UTF-8 input is refused, not mangled.** A latin-1 or binary file raises
  `ValueError` instead of being rewritten as UTF-8. Use a byte-level tool for
  those.
- **A refused edit leaves the file untouched.** Duplicate match, absent match,
  and missing file all fail before any write.

## Three things are called "edit"

The name is shared; the surfaces are not. Pick by where you are:

| surface | how you reach it | what it is |
|---|---|---|
| **this skill** | `await edit(path=…, old_str=…, new_str=…)` in a Python cell, or `!edit --path …` in a shell cell | the kernel's one-shot string replace; no modes, no streaming |
| **the native `edit` tool** | called by the model directly, gated by `editHybrid.enabled` | `pa-edit`'s full engine: `replace`, `patch`, `apply_patch`, `hashline`, `sloppy`; multi-file, atomic, precondition-checked |
| **the engine** | `pa_edit::Session` (Rust) | what the native tool drives; `pa-edit` crate |

**Prefer the native tool** for anything beyond a single exact replacement — it
carries modes, file preconditions, and post-write LSP diagnostics. This skill
exists for the kernel path, where only a one-shot replace is available.

This skill duplicates a slice of the engine. When the two disagree, the engine
is the reference: it detects the line ending with `detect_line_ending` and
writes it back. Keep this skill in step.
