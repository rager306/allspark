# pa-edit

The edit engine: five modes over one session, staging, and preview machinery.

**FORK-ONLY.** Ported from oh-my-pi's `pi-edit`; do not push upstream.

## Modes

| Mode | Input | Use |
| --- | --- | --- |
| `replace` | `path` + `edits[]` (`oldText`/`newText`) | the default; exact-match with fuzzy whitespace tolerance |
| `patch` | raw payload | structured hunks |
| `apply_patch` | raw payload (Lark grammar) | the Codex patch dialect |
| `hashline` | raw payload (Lark grammar) | snapshot-tagged line ops, stale-tag recovery |
| `sloppy` | raw payload (Lark grammar) | last resort when the exact text cannot be matched |

## Guarantees

- **Unique match or an error.** An ambiguous `oldText` is refused with the
  occurrence list, never silently applied to the first hit.
- **Stale snapshot recovery.** A `hashline` edit carrying an old tag is rebased
  onto the current text when the retained snapshot allows it, instead of
  failing or corrupting the file.
- **No-op loop detection.** Repeating the same payload without a commit is
  counted and surfaced.
- **Line endings and BOM preserved.** `detect_line_ending` reads the file's own
  ending and writes it back, so a one-line edit is a one-line diff.

## Consumers

`pa-core::tools::edit_hybrid` is the tool surface; the kernel-side `edit` skill
(`skills/edit/`) duplicates a slice of `replace` and must match its line-ending
contract.
