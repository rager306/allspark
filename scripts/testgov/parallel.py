"""Parallel runner over check kinds, and the shared-index plumbing.

The unit of parallelism is a CHECK, not a file: checks are 15 independent pure functions over a
shared read-only index, so they are embarrassingly parallel and need no lock. Files inside a check
are read from the shared tree index, which is built once.

Why threads and not processes: the work is I/O (`read_text`) plus regex, both of which release the
GIL in CPython. On the free-threaded build (`3.15t`) they genuinely overlap; on a GIL build the
same code is correct and simply serialised, so the tool never requires the free-threaded
interpreter — it exploits it when present.
"""

from __future__ import annotations

import os
import sys
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass, field
from pathlib import Path

from .protocol import Check, CheckResult, Context, Finding, Severity
from .tree import TreeIndex, walk


@dataclass
class RunResult:
    results: list[CheckResult] = field(default_factory=list)
    machine_findings: list[Finding] = field(default_factory=list)
    terminal: str = "fail"
    wall_ms: int = 0

    @property
    def high(self) -> int:
        return sum(result.high_count for result in self.results) + sum(
            1 for finding in self.machine_findings if finding.severity is Severity.HIGH
        )

    @property
    def low(self) -> int:
        return sum(result.low_count for result in self.results)


def default_jobs() -> int:
    """One worker per check is the cap; more workers than checks buys nothing."""
    override = os.environ.get("TESTGOV_JOBS")
    if override:
        try:
            return max(1, int(override))
        except ValueError:
            pass
    return min(8, os.cpu_count() or 1)


def is_free_threaded() -> bool:
    return bool(getattr(sys, "_is_gil_enabled", lambda: True)() is False)


def run_checks_parallel(
    checks: list[type[Check]],
    ctx: Context,
    tree: TreeIndex,
    *,
    jobs: int | None = None,
) -> list[CheckResult]:
    """Run the given checks concurrently. Order of results follows the input order."""
    if not checks:
        return []
    workers = min(jobs or default_jobs(), len(checks))
    results: dict[str, CheckResult] = {}
    if workers <= 1:
        for check_cls in checks:
            results[check_cls.name] = check_cls().run(ctx, tree=tree)
        return [results[check_cls.name] for check_cls in checks]
    with ThreadPoolExecutor(max_workers=workers, thread_name_prefix="testgov") as pool:
        # `tree` is keyword-only on Check.run; passing it positionally raised
        # "takes 2 positional arguments but 3 were given" for every check.
        futures = {
            pool.submit(check_cls().run, ctx, tree=tree): check_cls
            for check_cls in checks
        }
        for future in as_completed(futures):
            check_cls = futures[future]
            try:
                results[check_cls.name] = future.result()
            except Exception as error:  # noqa: BLE001 - a crashing check is a finding, not a crash
                results[check_cls.name] = CheckResult(
                    check_cls.name,
                    findings=[Finding(
                        check_cls.name, Severity.HIGH, "check-panicked",
                        f"{check_cls.name} raised {type(error).__name__}: {error}")],
                )
    return [results[check_cls.name] for check_cls in checks]


def prepare(root: Path) -> TreeIndex:
    """The one walk. Every check receives this instead of walking for itself."""
    return walk(root)
