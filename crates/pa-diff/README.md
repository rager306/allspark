# pa-diff

`FORK-ONLY` — never pushed upstream.

Line and word diff primitives shared by the edit engine and the terminal UI.

**FORK-ONLY.**

## Scope

- Line-level and word-level diff generation.
- Unified-diff rendering used by the `edit` tool's preview.

## Why one implementation

The edit engine renders a preview with this crate and the TUI renders the same
edit to the user. Two diff implementations would let the preview show one change
and the screen another; one crate makes that impossible.

## Consumers

- `pa-edit` — edit previews.
- `pa-tui` — rendering an applied edit.
