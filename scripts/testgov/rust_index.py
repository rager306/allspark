"""One pass over every .rs file: the facts a check needs about test functions.

Why a real index instead of a regex per check: `_prove_named_test` in the reference
implementation (kutha_gov/kinds.py:1434) has to distinguish a test from a helper, an
`#[ignore]`d test from an active one, and a nested module's test from a top-level one.
Doing that per check means four passes and four chances to disagree.

## Algorithm, and why (measured 2026-10-24, 1 574 files / 26 317 `fn` sites)

Cost breakdown of the naive version: `read_text` 0.15 s, `splitlines` 0.34 s, the anchored
multiline `fn` regex **1.08 s** — the regex was 65% of the index, and the whole governor was
30.6 s. Two changes, each measured against the previous one:

1. **O(n^2) removed.** The old code ran `text.count("\\n", 0, start)` and
   `sum(re.finditer(r"^}", text[:start]))` *per function*, i.e. a full scan and a slice copy per
   hit. 19 083 functions on 1 589 files. Replaced by a `line_starts` table + `bisect`, and the
   function body is no longer extracted at all — no check reads it.
   6.75 s -> 1.94 s.

2. **Literal prefilter before the engine.** This is the ripgrep lesson (`rg` is 5-13x `grep`
   because a literal byte scan rejects almost everything before the regex engine runs): most
   lines are not declarations. Per line, `"fn" in line` is a C-level scan; the anchored regex
   runs only on candidates. 1.08 s -> 0.25 s for the same result.
   1.94 s -> 0.55 s.

The prefilter is written against the *shape* `[visibility] [async] fn name`, not against an
enumerated list of visibility spellings: an earlier attempt listed `pub fn`/`async fn`/`pub(crate) fn`
and silently lost 962 functions, 868 of them real (`pub(crate) async fn ...`). A prefilter that
is faster and wrong is worse than no prefilter.
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

# Runs only on lines the literal prefilter accepted.
_DECL = re.compile(r"^(?P<prefix>(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?)fn\s+(?P<name>[a-z_][a-z0-9_]*)")
_ATTR_TEST = re.compile(r"#\[(?:tokio::)?test\b")
_ATTR_IGNORE = re.compile(r"#\[ignore\b")


@dataclass(frozen=True, slots=True)
class RustFn:
    """One definition. Deliberately carries no body: no check reads it, and extracting it was
    the O(n^2) term in the previous index."""

    name: str
    file: str
    line: int
    is_test: bool
    ignored: bool

    def qualifies_as_test(self) -> bool:
        """A test the runner will actually execute: attributed and not ignored."""
        return self.is_test and not self.ignored


def _declaration_name(line: str) -> str | None:
    """The fn name when `line` is a declaration, else None.

    A line can be indented (impl block, module) but a declaration never has other code before
    `fn`; the prefilter therefore checks `fn ` as a substring and lets the anchored regex decide.
    """
    stripped = line.lstrip()
    if "fn " not in stripped and "fn\\t" not in stripped:
        return None
    match = _DECL.match(stripped)
    if match is None:
        return None
    return match.group("name")


def index_rust(root: Path, *, subdir: str = "crates") -> dict[str, list[RustFn]]:
    """Map fn name -> every definition of it, across the workspace."""
    index: dict[str, list[RustFn]] = {}
    base = root / subdir
    if not base.is_dir():
        return index
    for path in base.rglob("*.rs"):
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        if "fn " not in text and "fn\\t" not in text:
            continue
        rel = path.relative_to(root).as_posix()
        lines = text.splitlines()
        for number, line in enumerate(lines, start=1):
            name = _declaration_name(line)
            if name is None:
                continue
            # The attribute block is the contiguous run of `#[...]` lines immediately above.
            is_test = ignored = False
            cursor = number - 1
            while cursor > 0:
                previous = lines[cursor - 1].strip()
                if previous.startswith("#[") or previous.startswith("#!["):
                    if _ATTR_TEST.search(previous):
                        is_test = True
                    if _ATTR_IGNORE.search(previous):
                        ignored = True
                    cursor -= 1
                    continue
                break
            index.setdefault(name, []).append(
                RustFn(name=name, file=rel, line=number, is_test=is_test, ignored=ignored)
            )
    return index


def test_names(index: dict[str, list[RustFn]]) -> set[str]:
    """Every fn that is an attributed, non-ignored test anywhere in the workspace."""
    return {
        name
        for name, defs in index.items()
        if any(entry.qualifies_as_test() for entry in defs)
    }
