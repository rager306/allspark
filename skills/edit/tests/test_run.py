"""Regression tests for the kernel-side edit skill.

The skill duplicates a slice of the native pa-edit engine, so it must hold the
same line-ending contract: a one-line edit to a CRLF file must not rewrite every
line ending in it, because that turns a targeted change into a whole-file diff.
"""

from __future__ import annotations

import asyncio
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from edit import run  # noqa: E402


def edit(path: Path, old: str, new: str) -> str:
    return asyncio.run(run(path=str(path), old_str=old, new_str=new))


def test_lf_file_keeps_lf(tmp_path: Path) -> None:
    target = tmp_path / "f.py"
    target.write_bytes(b"def f():\n    return 1\n")
    edit(target, "return 1", "return 2")
    assert target.read_bytes() == b"def f():\n    return 2\n"


def test_crlf_file_keeps_crlf(tmp_path: Path) -> None:
    """The bug this test exists for: a targeted edit rewrote every ending."""
    target = tmp_path / "f.py"
    target.write_bytes(b"def f():\r\n    return 1\r\n")
    edit(target, "return 1", "return 2")
    data = target.read_bytes()
    assert data == b"def f():\r\n    return 2\r\n", data
    assert b"\n" not in data.replace(b"\r\n", b""), "no bare LF may appear"


def test_crlf_file_accepts_an_lf_needle(tmp_path: Path) -> None:
    """An `old_str` written with LF still matches a CRLF file."""
    target = tmp_path / "f.py"
    target.write_bytes(b"def f():\r\n    return 1\r\n")
    edit(target, "def f():\n    return 1", "def f():\n    return 3")
    assert target.read_bytes() == b"def f():\r\n    return 3\r\n"


def test_bom_is_preserved(tmp_path: Path) -> None:
    target = tmp_path / "f.py"
    target.write_bytes("\ufeffdef f():\n    return 1\n".encode())
    edit(target, "return 1", "return 2")
    data = target.read_bytes()
    assert data.startswith(b"\xef\xbb\xbf"), "the BOM must survive"
    assert b"return 2" in data


def test_duplicate_match_is_refused(tmp_path: Path) -> None:
    target = tmp_path / "f.py"
    target.write_bytes(b"x = 1\nx = 1\n")
    with pytest.raises(ValueError, match="need exactly 1"):
        edit(target, "x = 1", "x = 2")
    # The refusal must leave the file untouched.
    assert target.read_bytes() == b"x = 1\nx = 1\n"


def test_absent_match_is_refused(tmp_path: Path) -> None:
    target = tmp_path / "f.py"
    target.write_bytes(b"x = 1\n")
    with pytest.raises(ValueError, match="not found"):
        edit(target, "nope", "y")
    assert target.read_bytes() == b"x = 1\n"


def test_missing_file_is_refused(tmp_path: Path) -> None:
    with pytest.raises(FileNotFoundError):
        edit(tmp_path / "absent.py", "a", "b")


def test_non_utf8_file_is_refused_not_mangled(tmp_path: Path) -> None:
    """A binary or latin-1 file must error, never be rewritten as UTF-8."""
    target = tmp_path / "latin.py"
    target.write_bytes(b"# caf\xe9\ndef f():\n    return 1\n")
    with pytest.raises(ValueError, match="not valid UTF-8"):
        edit(target, "return 1", "return 2")
    assert target.read_bytes() == b"# caf\xe9\ndef f():\n    return 1\n"


def test_only_the_first_of_several_files_is_touched(tmp_path: Path) -> None:
    """A sibling file with the same text must not change."""
    target = tmp_path / "a.py"
    sibling = tmp_path / "b.py"
    target.write_bytes(b"return 1\n")
    sibling.write_bytes(b"return 1\n")
    edit(target, "return 1", "return 2")
    assert target.read_bytes() == b"return 2\n"
    assert sibling.read_bytes() == b"return 1\n"
