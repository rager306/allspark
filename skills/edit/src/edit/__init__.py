"""Exact single-occurrence string replacement for existing files."""

from __future__ import annotations

from pathlib import Path

# The native pa-edit engine detects the file's line ending and writes it back,
# so a CRLF file stays CRLF. This kernel-side helper read and wrote with
# ``read_text``/``write_text``, which silently normalized every CRLF to LF: a
# one-line edit rewrote the whole file's line endings and showed up as a
# full-file diff in git. Match the engine's behaviour.
_LINE_ENDINGS = ("\r\n", "\n", "\r")


def _detect_line_ending(content: str) -> str:
    """Return the file's dominant line ending, defaulting to LF.

    The first ending wins, which is what the native engine does: a file that
    starts with CRLF is a CRLF file even if a stray LF appears later.
    """
    best = "\n"
    best_at = len(content)
    for ending in _LINE_ENDINGS:
        at = content.find(ending)
        if at != -1 and at < best_at:
            best = ending
            best_at = at
    return best


async def run(path: str, old_str: str, new_str: str) -> str:
    """Replace a unique string in a file.

    ``old_str`` must appear exactly once in the file at ``path``; that match is
    replaced with ``new_str`` and the file is written back in place. Prefer this
    over rewriting a whole file for targeted edits.

    The file's line endings are preserved: a CRLF file stays CRLF, so a
    one-line edit does not appear as a whole-file rewrite in version control.

    Args:
        path: File to edit, relative to the working directory, absolute, or
            `~`-prefixed (the leading `~`/`~user` is expanded to the home dir).
        old_str: Exact text to find. Must occur exactly once in the file.
        new_str: Replacement text.

    Returns:
        A short confirmation message.

    Raises:
        FileNotFoundError: If ``path`` does not exist.
        ValueError: If ``old_str`` is absent or matches more than once, or if
            the file is not valid UTF-8.
    """
    filepath = Path(path).expanduser()
    if not filepath.exists():
        raise FileNotFoundError(f"{path} not found")
    try:
        # `newline=""` disables universal-newline translation: without it the
        # read itself turns CRLF into LF and the line ending is gone before any
        # of the logic below can preserve it.
        with filepath.open(encoding="utf-8", newline="") as handle:
            raw = handle.read()
    except UnicodeDecodeError as error:
        # A binary or non-UTF-8 file is not something a string replace can
        # handle safely; say so instead of writing back a mangled copy.
        raise ValueError(
            f"{path} is not valid UTF-8 ({error}); use a byte-level tool for binary files"
        ) from error

    # Compare and replace on LF-normalized text so an `old_str` written with
    # LF still matches a CRLF file, then restore the file's own ending.
    line_ending = _detect_line_ending(raw)
    content = raw if line_ending == "\n" else raw.replace("\r\n", "\n").replace("\r", "\n")
    needle = old_str if line_ending == "\n" else old_str.replace("\r\n", "\n").replace("\r", "\n")
    replacement = new_str if line_ending == "\n" else new_str.replace("\r\n", "\n").replace("\r", "\n")

    count = content.count(needle)
    if count == 0:
        raise ValueError(f"string not found in {path}")
    if count > 1:
        raise ValueError(
            f"found {count} occurrences in {path}, need exactly 1 — "
            "widen the snippet to make it unique"
        )
    match_index = content.index(needle)
    start_line = content.count("\n", 0, match_index) + 1
    updated = content.replace(needle, replacement, 1)
    if line_ending != "\n":
        updated = updated.replace("\n", line_ending)
    filepath.write_text(updated, encoding="utf-8", newline="")

    resolved_path = str(filepath.resolve())
    _emit_diff(resolved_path, old_str, new_str, start_line)
    return f"Edited {resolved_path}"

# Keep in sync with DIFF_DISPLAY_MIME in src/core/kernel/index.ts.
_DIFF_DISPLAY_MIME = "application/vnd.prime-agent.diff+json"


def _emit_diff(path: str, old_str: str, new_str: str, start_line: int) -> None:
    """Stream a diff to the host as a display event; best-effort outside the kernel."""
    try:
        from rlm import emit

        emit(
            {
                _DIFF_DISPLAY_MIME: {
                    "path": path,
                    "old_str": old_str,
                    "new_str": new_str,
                    "start_line": start_line,
                },
                "text/plain": f"Edited {path}",
            }
        )
    except Exception:
        pass
