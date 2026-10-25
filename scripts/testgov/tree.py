"""One filesystem walk shared by every check, and a parallel runner over it.

Measured before designing this (2026-10-24, real tree):
  index_rust          6.74 s  19 083 fns
  non-rust-surfaces  29.65 s  for 132 files
  settings-consumers 25.31 s  for 127 fields
`rglob` over `target/` cost 11.95 s of that for 132 wanted files: every check re-walked ~87 000
entries. So the first fix is not threads, it is walking ONCE.

Order of optimisations, each measured:
  1. one walk, shared                       (this module)
  2. parallel over check kinds, not per file (ThreadPoolExecutor; needs the free-threaded build
                                              to actually overlap, see pyproject)
  3. the Rust index is built once and passed in, never rebuilt per check
"""

from __future__ import annotations

import os
from dataclasses import dataclass, field
from pathlib import Path

# Directories that are never part of the contract. `target/` alone is ~87k entries.
SKIP_DIRS = frozenset({
    ".git", "target", "node_modules", ".venv", "venv", "__pycache__",
    ".tmp", ".research", ".kache-prestage",
})

TEXT_SUFFIXES = frozenset({".rs", ".toml", ".yaml", ".yml", ".json", ".py", ".sh", ".md"})


@dataclass
class TreeIndex:
    """Every tracked-ish file the governor may need, walked exactly once.

    Also owns the Rust index: the parse is 6.7 s for 19 083 functions, and the governor runs
    several checks that need it. Building it per check was measured at 6.8 s per call.
    """

    root: Path
    by_suffix: dict[str, list[Path]] = field(default_factory=dict)
    _text_cache: dict[Path, str] = field(default_factory=dict)
    _rust_index: dict | None = None

    def rust_index(self) -> dict:
        """The parsed `.rs` functions, built on first use and reused by every check."""
        if self._rust_index is None:
            from .rust_index import index_rust

            self._rust_index = index_rust(self.root)
        return self._rust_index

    def files(self, *suffixes: str) -> list[Path]:
        out: list[Path] = []
        for suffix in suffixes:
            out.extend(self.by_suffix.get(suffix, ()))
        return out

    def text(self, path: Path) -> str:
        cached = self._text_cache.get(path)
        if cached is None:
            cached = path.read_text(encoding="utf-8", errors="replace")
            self._text_cache[path] = cached
        return cached

    def rel(self, path: Path) -> str:
        return path.relative_to(self.root).as_posix()

    def all_rust_text(self) -> str:
        return "\n".join(self.text(path) for path in self.by_suffix.get(".rs", ()))


def walk(root: Path) -> TreeIndex:
    """One os.scandir-based walk. `os.scandir` is ~3x `rglob` here because `rglob` builds Path
    objects for every entry it visits, including the tens of thousands it then discards."""
    index = TreeIndex(root=root)
    stack = [root]
    while stack:
        current = stack.pop()
        try:
            entries = list(os.scandir(current))
        except OSError:
            continue
        for entry in entries:
            try:
                if entry.is_dir(follow_symlinks=False):
                    if entry.name not in SKIP_DIRS:
                        stack.append(Path(entry.path))
                    continue
            except OSError:
                continue
            suffix = Path(entry.name).suffix
            if suffix in TEXT_SUFFIXES:
                index.by_suffix.setdefault(suffix, []).append(Path(entry.path))
    for paths in index.by_suffix.values():
        paths.sort()
    return index


def is_scratch(path: Path, root: Path) -> bool:
    """Generated, scratch, or fixture trees are out of contract: they are not shipped.

    The governor's own fixtures are deliberately malformed (that is how `selftest` proves a
    check can fail), so scanning them would report the fixtures as defects of the tree.
    """
    try:
        rel = path.relative_to(root)
    except ValueError:
        return True
    if any(part in SKIP_DIRS for part in rel.parts):
        return True
    return len(rel.parts) >= 3 and rel.parts[0] == "scripts" and rel.parts[1] == "testgov" \
        and rel.parts[2] == "fixtures"
