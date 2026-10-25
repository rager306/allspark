"""Resolve oracle names without indexing the whole workspace.

Exa research (ast-grep's "Optimize ast-grep to get 10X faster" post, and the trigram-index
literature: Google Code Search → Zoekt → Livegrep → qndx) gives two transferable rules:

  1. **Duplicate traversal is the cost.** ast-grep's own bottleneck was matching the whole AST
     once per rule; fixing it was the single biggest win. Here the same shape appeared: three
     checks each built their own Rust index.
  2. **Index once, then O(candidates).** A trigram index turns "scan the corpus" into "scan the
     files that can match". The cheap version of that, for a *small* query set, is a literal
     prefilter: only parse files whose text contains one of the names asked about.

Measured on this tree (2026-10-24):
    full index (19 083 names / 25 844 sites)          0.82 s
    targeted resolution of the matrix's 5 oracle names 0.22 s
The matrix asks about FEW names; the full index is only needed by `untracked-oracle`, which wants
every test name. So the full index is built lazily, on first request, and only that check asks.
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

from .rust_index import RustFn, _declaration_name


def _attributes_above(lines: list[str], number: int) -> tuple[bool, bool]:
    """(is_test, ignored) from the contiguous `#[...]` block above a 1-based line."""
    is_test = ignored = False
    cursor = number - 1
    while cursor > 0:
        previous = lines[cursor - 1].strip()
        if previous.startswith("#[") or previous.startswith("#!["):
            if "#[test]" in previous or "#[tokio::test]" in previous:
                is_test = True
            if "#[ignore" in previous:
                ignored = True
            cursor -= 1
            continue
        break
    return is_test, ignored


def resolve_names(tree, wanted: set[str]) -> dict[str, list[RustFn]]:
    """Definitions of `wanted`, found by scanning only files that contain one of the names."""
    found: dict[str, list[RustFn]] = {}
    if not wanted:
        return found
    for path in tree.files(".rs"):
        text = tree.text(path)
        # Literal prefilter: skip files that cannot mention any wanted name at all.
        if not any(name in text for name in wanted):
            continue
        rel = tree.rel(path)
        lines = text.splitlines()
        for number, line in enumerate(lines, start=1):
            name = _declaration_name(line)
            if name is None or name not in wanted:
                continue
            is_test, ignored = _attributes_above(lines, number)
            found.setdefault(name, []).append(
                RustFn(name=name, file=rel, line=number, is_test=is_test, ignored=ignored)
            )
    return found
