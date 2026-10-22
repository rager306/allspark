#!/usr/bin/env python3
"""Check that the Verus mirrors still agree with the sources they mirror.

A mirror is a copy of a function's logic written for the verifier: `verus/*.rs`
restates `crates/*/src/*.rs` in a form Verus accepts, and every mirror opens with

    // MIRROR OF: crates/pa-mem/src/ptr48.rs (...)

Nothing enforced that comment. A proof about a drifted mirror is worse than no
proof: it is a false assurance, and the failure is silent. This script makes the
cheapest drift impossible to miss — the constants both files must agree on.

It does not prove the code matches. It catches the case that actually happens:
someone changes a constant in the source and not in the mirror.

Usage:
    scripts/verus-mirror-check.py          # exit 1 on any drift
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Mirrors verified here, with the constants each must agree on with its source.
# A constant listed here is compared *after* the source's arithmetic is resolved
# (`(1u64 << PTR_BITS) - 1` becomes a number), so a derived constant is checked
# against the real value rather than against its spelling.
MIRRORS = [
    (
        "verus/pa_mem_ptr48.rs",
        "crates/pa-mem/src/ptr48.rs",
        ["MAX_INDEX"],
    ),
]


def read_constants(path: Path) -> dict[str, int]:
    """Every `pub const NAME: T = EXPR;` that can be resolved to an integer."""
    text = path.read_text(encoding="utf-8")
    raw: dict[str, str] = {}
    for match in re.finditer(r"pub const (\w+)\s*:\s*\w+\s*=\s*([^;]+);", text, re.S):
        raw[match.group(1)] = match.group(2).strip()
    resolved: dict[str, int] = {}
    for _ in range(10):
        progress = False
        for name, expr in raw.items():
            if name in resolved:
                continue
            # Rust writes `1u64`, `0xFFu8`; Python wants the bare number. Strip the
            # suffix with or without a following `_` or space, since `1u64 <<` is
            # the form this repository actually uses.
            candidate = re.sub(r"(\d)(u64|u32|usize|i64|i32|u8)(?![A-Za-z0-9_])", r"\1", expr)
            # Underscores are digit separators in Rust literals but part of
            # identifier names, so strip them only from numeric runs.
            candidate = re.sub(r"(?<=\d)_(?=\d)", "", candidate)
            for known, value in resolved.items():
                candidate = re.sub(rf"\b{known}\b", str(value), candidate)
            if re.search(r"\b[A-Za-z_]\w*\b", candidate):
                continue
            try:
                resolved[name] = int(eval(candidate, {"__builtins__": {}}, {}))
                progress = True
            except Exception:
                continue
        if not progress:
            break
    return resolved


def main() -> int:
    failed = 0
    for mirror_rel, source_rel, names in MIRRORS:
        mirror_path, source_path = ROOT / mirror_rel, ROOT / source_rel
        if not mirror_path.exists() or not source_path.exists():
            print(f"verus-mirror-check: missing {mirror_rel} or {source_rel}", file=sys.stderr)
            failed += 1
            continue
        mirror = read_constants(mirror_path)
        source = read_constants(source_path)
        for name in names:
            have, want = mirror.get(name), source.get(name)
            if have is None or want is None:
                print(
                    f"verus-mirror-check: {name} not resolvable "
                    f"(mirror={have!r} source={want!r}) in {mirror_rel}",
                    file=sys.stderr,
                )
                failed += 1
                continue
            if have != want:
                print(
                    f"verus-mirror-check: DRIFT {name}: {mirror_rel} says {have}, "
                    f"{source_rel} says {want}",
                    file=sys.stderr,
                )
                failed += 1
            else:
                print(f"verus-mirror-check: {name} = {have} (agrees)")
    if failed:
        print(f"verus-mirror-check: {failed} drift(s) — update the mirror with the source", file=sys.stderr)
        return 1
    print("verus-mirror-check: mirrors agree with their sources")
    return 0


if __name__ == "__main__":
    sys.exit(main())
