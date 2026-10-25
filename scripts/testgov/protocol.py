"""Check protocol. HIGH fails CI; LOW advises. Two severities, never more."""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import StrEnum
from pathlib import Path


class Severity(StrEnum):
    HIGH = "high"
    LOW = "low"


@dataclass(frozen=True)
class Finding:
    check: str
    severity: Severity
    category: str
    message: str
    file: str | None = None
    line: int | None = None

    def format(self) -> str:
        tag = "HIGH" if self.severity is Severity.HIGH else "low"
        loc = "-"
        if self.file and self.line:
            loc = f"{self.file}:{self.line}"
        elif self.file:
            loc = self.file
        return f"[{tag}] {loc} ({self.check}/{self.category}) {self.message}"


@dataclass
class CheckResult:
    check: str
    findings: list[Finding] = field(default_factory=list)
    scanned: int = 0
    note: str = ""

    @property
    def passed(self) -> bool:
        """HIGH findings are the only failure."""
        return self.high_count == 0

    @property
    def high_count(self) -> int:
        return sum(1 for finding in self.findings if finding.severity is Severity.HIGH)

    @property
    def low_count(self) -> int:
        return sum(1 for finding in self.findings if finding.severity is Severity.LOW)


@dataclass(frozen=True)
class Context:
    root: Path
    changed_paths: frozenset[str] | None = None

    def read(self, rel_path: str) -> str | None:
        """File content relative to the root, or None when absent."""
        path = self.root / rel_path
        if not path.is_file():
            return None
        return path.read_text(encoding="utf-8", errors="replace")


class Check:
    """One named check.

    `tree` is the shared, read-only walk of the repository. A check must NOT walk for itself:
    measured 2026-10-24, three checks independently walking the tree cost 55 s of 58 s of wall
    clock, and every one of them visited ~87 000 entries to find 132 files.
    """

    name: str = ""
    description: str = ""

    def run(self, ctx: Context, *, tree=None) -> CheckResult:
        raise NotImplementedError
