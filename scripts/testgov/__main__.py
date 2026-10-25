"""CLI: ci | list | explain | map | selftest | json.

`selftest` is the load-bearing command: it runs every check against a fixture that must
make it emit a HIGH. A check that cannot fail is the exact defect class this governor
exists to catch, so a green selftest is a precondition for trusting `ci`.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from .dictionary import MATRIX_PATH, load_matrix
from .kinds import ALL_CHECKS, by_name
from .protocol import CheckResult, Context, Severity


def _free_threaded() -> bool:
    from .parallel import is_free_threaded

    return is_free_threaded()


def _root(explicit: Path | None) -> Path:
    if explicit is not None:
        return explicit.resolve()
    for candidate in (Path.cwd(), *Path.cwd().parents):
        if (candidate / "Cargo.toml").is_file() and (candidate / "crates").is_dir():
            return candidate
    return Path.cwd()


def _run(checks, ctx: Context) -> tuple[list[CheckResult], int]:
    results: list[CheckResult] = []
    for check_cls in checks:
        check = check_cls()
        results.append(check.run(ctx))
    return results, 1 if any(result.high_count for result in results) else 0


def _report(results: list[CheckResult], as_json: bool) -> None:
    if as_json:
        payload = [
            {
                "check": result.check,
                "passed": result.passed,
                "high": result.high_count,
                "low": result.low_count,
                "scanned": result.scanned,
                "note": result.note,
                "findings": [finding.__dict__ | {"severity": str(finding.severity)} for finding in result.findings],
            }
            for result in results
        ]
        print(json.dumps(payload, indent=2, default=str))
        return
    for result in results:
        for finding in result.findings:
            print(finding.format())
        state = "PASS" if result.passed else "FAIL"
        note = f" — {result.note}" if result.note else ""
        print(f"{state} {result.check} (scanned {result.scanned}, {result.high_count} high, {result.low_count} low){note}")
    high = sum(result.high_count for result in results)
    low = sum(result.low_count for result in results)
    print(f"\n{high} high, {low} low across {len(results)} checks")


def command_ci(args, root: Path) -> int:
    """Walk the FSM. The machine decides which checks run and in what order; a missing
    dictionary, an unknown kind or a missing transition is itself a HIGH finding."""
    from .fsm import load_fsm, validate, walk

    from .parallel import prepare
    from .tree import walk as _walk

    ctx = Context(root=root, changed_paths=_changed(args))
    fsm = load_fsm(root)
    if not fsm.ok:
        print(f"[HIGH] - (fsm/unreadable) {fsm.error}")
        return 1
    machine_findings = validate(fsm)
    checks = by_name()
    # One walk of the repository, shared by every check. Measured: without it the tree-walking
    # checks spent 55 s of 58 s re-visiting ~87 000 entries for 132 wanted files.
    tree = prepare(root)
    results, walk_findings, terminal = walk(
        fsm, ctx, checks, _selftest_problems, tree=tree, jobs=args.jobs,
    )
    machine_findings.extend(walk_findings)
    if args.format == "json":
        payload = {
            "terminal": terminal,
            "machine": [
                finding.__dict__ | {"severity": str(finding.severity)}
                for finding in machine_findings
            ],
            "checks": [
                {
                    "check": result.check,
                    "passed": result.passed,
                    "high": result.high_count,
                    "low": result.low_count,
                    "scanned": result.scanned,
                    "findings": [
                        finding.__dict__ | {"severity": str(finding.severity)}
                        for finding in result.findings
                    ],
                }
                for result in results
            ],
        }
        print(json.dumps(payload, indent=2, default=str))
    else:
        for finding in machine_findings:
            print(finding.format())
        _report(results, as_json=False)
        if args.timing:
            print(f"workers: {args.jobs or 'auto'} | free-threaded: {_free_threaded()}")
        print(f"fsm: terminal {terminal}")
    high = sum(result.high_count for result in results) + sum(
        1 for finding in machine_findings if finding.severity is Severity.HIGH
    )
    # First-contact injection: a red run carries its own reader's guide, so
    # nobody meets this tool without the fix workflow in front of them.
    if high and args.format != "json":
        from .onboarding import red_guide

        print(red_guide())
    return 1 if high else 0


def _changed(args) -> frozenset[str] | None:
    if not args.changed:
        return None
    import subprocess
    try:
        completed = subprocess.run(
            ["git", "diff", "--name-only", "HEAD"],
            capture_output=True, text=True, check=True,
        )
    except (OSError, subprocess.CalledProcessError):
        return None
    return frozenset(line.strip() for line in completed.stdout.splitlines() if line.strip())


def command_list(args, root: Path) -> int:
    matrix = load_matrix(root)
    if not matrix.ok:
        print(matrix.error, file=sys.stderr)
        return 1
    print(f"{'id':40} {'plane':8} {'layer':11} {'status':16} oracles")
    for row in matrix.rows:
        print(f"{row.id:40} {str(row.get('plane')):8} {str(row.get('layer')):11} "
              f"{str(row.get('status')):16} {len(row.oracles)}")
    return 0


def command_explain(args, root: Path) -> int:
    matrix = load_matrix(root)
    if not matrix.ok:
        print(matrix.error, file=sys.stderr)
        return 1
    for row in matrix.rows:
        if row.id != args.name:
            continue
        print(json.dumps(row.raw, indent=2, default=str))
        print(f"\nmatrix line: {row.line}")
        return 0
    print(f"no row named {args.name!r}", file=sys.stderr)
    return 1


def command_map(args, root: Path) -> int:
    matrix = load_matrix(root)
    if not matrix.ok:
        print(matrix.error, file=sys.stderr)
        return 1
    from collections import Counter
    planes = Counter(str(row.get("plane")) for row in matrix.rows)
    layers = Counter(str(row.get("layer")) for row in matrix.rows)
    statuses = Counter(str(row.get("status")) for row in matrix.rows)
    axes = Counter(axis for row in matrix.rows for axis in row.axis_names)
    if args.format == "json":
        print(json.dumps({
            "planes": planes, "layers": layers, "statuses": statuses, "axes": axes,
            "rows": len(matrix.rows),
        }, indent=2, default=dict))
        return 0
    print("planes:", dict(planes))
    print("layers:", dict(layers))
    print("statuses:", dict(statuses))
    print("axes:", dict(axes))
    print(f"rows: {len(matrix.rows)}  (a map of oracles, not a coverage score)")
    return 0


def _selftest_problems(ctx: Context) -> list[str]:
    """The set of checks that pass their own fixture. Called from the FSM's `run_selftest`."""
    fixtures = Path(__file__).parent / "fixtures"
    problems: list[str] = []
    if not fixtures.is_dir():
        return [f"no fixtures directory at {fixtures}"]
    for check_cls in ALL_CHECKS:
        fixture = fixtures / check_cls.name
        if not fixture.is_dir():
            problems.append(f"{check_cls.name} has no fixture dir (cannot prove it fails)")
            continue
        changed_file = fixture / "changed.txt"
        changed = None
        if changed_file.is_file():
            changed = frozenset(
                line.strip() for line in changed_file.read_text().splitlines() if line.strip()
            )
        outcome = check_cls().run(Context(root=fixture, changed_paths=changed))
        if not outcome.findings:
            problems.append(f"{check_cls.name} PASSED its fixture — the check cannot fail")
    return problems




def command_learn(args, root: Path) -> int:
    """The scenario card: what this tool does, on demand."""
    from .onboarding import scenario_card

    print(scenario_card())
    return 0


def command_selftest(args, root: Path) -> int:
    """Every check must emit a finding on its fixture. A check that cannot fail fails here."""
    fixtures = Path(__file__).parent / "fixtures"
    problems = _selftest_problems(Context(root=root))
    for check_cls in ALL_CHECKS:
        fixture = fixtures / check_cls.name
        if not fixture.is_dir():
            continue
        if any(problem.startswith(check_cls.name) for problem in problems):
            print(f"selftest: {check_cls.name} PASSED its fixture — the check cannot fail")
        else:
            print(f"selftest: {check_cls.name} fails its fixture as required")
    for problem in problems:
        if "no fixture dir" in problem:
            print(f"selftest: {problem}")
    return 1 if problems else 0


COMMANDS = {
    "ci": command_ci,
    "list": command_list,
    "explain": command_explain,
    "map": command_map,
    "selftest": command_selftest,
    "learn": command_learn,
}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="testgov", description="ADR-0001 test-contract governor")
    parser.add_argument("--root", type=Path, default=None)
    parser.add_argument("--changed", action="store_true", help="ci: check only rows whose code axis changed")
    parser.add_argument("--format", choices=("text", "json"), default="text")
    parser.add_argument(
        "--jobs", type=int, default=None,
        help="parallel check workers (default: min(8, cpu); 1 = serial)",
    )
    parser.add_argument(
        "--timing", action="store_true", help="print per-check wall time",
    )
    parser.add_argument("command", nargs="?", default="ci", choices=tuple(COMMANDS))
    parser.add_argument("name", nargs="?", help="row id for explain")
    args = parser.parse_args(argv)
    root = _root(args.root)
    return COMMANDS[args.command](args, root)


if __name__ == "__main__":
    sys.exit(main())
