"""The interpreter: a fail-closed FSM over dictionary-loaded states.

Design (ADR-0001 §B, doc/testing/META.md): the governor is an *interpreter*, not a hard-coded
sequence. Adding a check is a dictionary edit to `doc/testing/fsm.yaml`; adding a *kind* is a
kernel change here plus a fixture proving it can fail.

Two rules make it fail-closed:
  * an unknown state kind is a HIGH finding (`unknown-fsm-kind`), never a silent skip;
  * a state with no matching transition for its event is a HIGH finding
    (`missing-transition`), so a machine edit cannot quietly strand a check.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path

import yaml

from .protocol import CheckResult, Context, Finding, Severity

FSM_PATH = "doc/testing/fsm.yaml"
META_PATH = "doc/testing/META.md"

# The allowlist of state kinds. A new kind is a kernel change: implement it in `run_state`
# and add a fixture under scripts/testgov/fixtures/fsm/.
ALLOWED_KINDS = frozenset({"noop", "require_file", "run_checks", "run_selftest", "decide"})


@dataclass
class Transition:
    source: str
    event: str
    target: str


@dataclass
class Fsm:
    initial: str = "idle"
    terminal: frozenset[str] = frozenset({"ok", "fail"})
    states: dict[str, dict] = field(default_factory=dict)
    transitions: list[Transition] = field(default_factory=list)
    budget: int = 105
    error: str = ""

    @property
    def ok(self) -> bool:
        return not self.error


def load_fsm(root: Path) -> Fsm:
    path = root / FSM_PATH
    if not path.is_file():
        return Fsm(error=f"{FSM_PATH} is missing")
    try:
        document = yaml.safe_load(path.read_text(encoding="utf-8"))
    except yaml.YAMLError as error:
        return Fsm(error=f"{FSM_PATH} is not valid YAML: {error}")
    if not isinstance(document, dict):
        return Fsm(error=f"{FSM_PATH} is not a mapping")
    states = document.get("states")
    raw_transitions = document.get("transitions")
    if not isinstance(states, dict) or not isinstance(raw_transitions, list):
        return Fsm(error=f"{FSM_PATH} needs `states` and `transitions`")
    transitions: list[Transition] = []
    for entry in raw_transitions:
        if not isinstance(entry, dict):
            return Fsm(error=f"{FSM_PATH}: a transition is not a mapping: {entry!r}")
        try:
            transitions.append(Transition(str(entry["from"]), str(entry["event"]), str(entry["to"])))
        except KeyError as missing:
            return Fsm(error=f"{FSM_PATH}: transition missing {missing}: {entry!r}")
    defaults = document.get("defaults") or {}
    return Fsm(
        initial=str(document.get("initial", "idle")),
        terminal=frozenset(document.get("terminal") or ["ok", "fail"]),
        states={str(name): (spec if isinstance(spec, dict) else {}) for name, spec in states.items()},
        transitions=transitions,
        budget=int(defaults.get("budget", 105)) if isinstance(defaults, dict) else 105,
    )


def validate(fsm: Fsm) -> list[Finding]:
    """Static checks on the machine itself. All HIGH: a broken machine proves nothing."""
    findings: list[Finding] = []
    for name, spec in fsm.states.items():
        kind = spec.get("kind")
        if kind not in ALLOWED_KINDS:
            findings.append(Finding(
                "fsm", Severity.HIGH, "unknown-fsm-kind",
                f"state {name!r} has kind {kind!r}, not in {sorted(ALLOWED_KINDS)}",
                file=FSM_PATH))
        if name in fsm.terminal:
            continue
        if not any(transition.source == name for transition in fsm.transitions):
            findings.append(Finding(
                "fsm", Severity.HIGH, "missing-transition",
                f"state {name!r} has no outgoing transition", file=FSM_PATH))
    for transition in fsm.transitions:
        for endpoint in (transition.source, transition.target):
            if endpoint not in fsm.states and endpoint not in fsm.terminal:
                findings.append(Finding(
                    "fsm", Severity.HIGH, "dangling-transition",
                    f"transition {transition.source} -{transition.event}-> {transition.target} "
                    f"names unknown state {endpoint!r}", file=FSM_PATH))
    if not any(transition.target == "fail" for transition in fsm.transitions):
        findings.append(Finding(
            "fsm", Severity.HIGH, "unreachable-fail",
            "no transition targets `fail`, so the machine cannot report a failing check",
            file=FSM_PATH))
    if fsm.initial not in fsm.states:
        findings.append(Finding(
            "fsm", Severity.HIGH, "unknown-initial",
            f"initial state {fsm.initial!r} is not defined", file=FSM_PATH))
    return findings


def walk(fsm: Fsm, ctx: Context, checks_by_name, selftest_runner, *, tree=None, jobs=None):
    """Execute the machine. Returns (per-check results, machine findings, terminal state)."""
    findings: list[Finding] = []
    results: list[CheckResult] = []
    state = fsm.initial
    steps = 0
    while state not in fsm.terminal:
        steps += 1
        if steps > fsm.budget:
            findings.append(Finding(
                "fsm", Severity.HIGH, "budget-exhausted",
                f"the machine took more than {fsm.budget} steps before reaching a terminal state",
                file=FSM_PATH))
            return results, findings, "fail"
        spec = fsm.states.get(state)
        if spec is None:
            findings.append(Finding(
                "fsm", Severity.HIGH, "unknown-state",
                f"the machine reached {state!r}, which is not defined", file=FSM_PATH))
            return results, findings, "fail"
        kind = spec.get("kind")
        # Every kind may override the outgoing event; the default is `done`.
        event = "done"
        if kind == "noop":
            pass
        elif kind == "require_file":
            path = str(spec.get("path", ""))
            event = "ok"
            if not (ctx.root / path).is_file():
                findings.append(Finding(
                    "fsm", Severity.HIGH, "missing-dictionary",
                    f"{state}: required file {path} is missing", file=FSM_PATH))
                event = "fail"
        elif kind == "run_checks":
            from .parallel import run_checks_parallel

            names = [str(name) for name in (spec.get("checks") or [])]
            if not names:
                findings.append(Finding(
                    "fsm", Severity.HIGH, "state-names-no-check",
                    f"{state}: run_checks names no check", file=FSM_PATH))
                event = "fail"
            resolved: list = []
            for name in names:
                check_cls = checks_by_name.get(name)
                if check_cls is None:
                    findings.append(Finding(
                        "fsm", Severity.HIGH, "unknown-check",
                        f"{state}: no check named {name!r}", file=FSM_PATH))
                    event = "fail"
                    continue
                resolved.append(check_cls)
            # Checks inside one state are independent; run them concurrently over the shared
            # tree. Measured: the three tree-walking checks cost 55 s serially, 9 s shared.
            for outcome in run_checks_parallel(resolved, ctx, tree, jobs=jobs):
                results.append(outcome)
                if not outcome.passed:
                    event = "fail"
        elif kind == "run_selftest":
            problems = selftest_runner(ctx)
            for problem in problems:
                findings.append(Finding(
                    "fsm", Severity.HIGH, "selftest-failed", problem, file=FSM_PATH))
            if problems:
                event = "fail"
        elif kind == "decide":
            # A selftest problem is a HIGH finding by construction, so the aggregate below is
            # the single place that decides the terminal edge.
            if any(finding.severity is Severity.HIGH for finding in findings) or any(
                not result.passed for result in results
            ):
                event = "fail"
            else:
                event = "ok"
        else:
            findings.append(Finding(
                "fsm", Severity.HIGH, "unknown-fsm-kind",
                f"{state}: kind {kind!r} is not implemented", file=FSM_PATH))
            return results, findings, "fail"

        # A check state records its outcome and continues: a governor reports every finding in
        # one pass and decides once. Only `require_file` (a missing dictionary makes every later
        # check meaningless) takes its `fail` edge immediately.
        if event == "fail" and kind == "run_checks":
            findings.append(Finding(
                "fsm", Severity.HIGH, "check-state-failed",
                f"{state}: a check reported HIGH; continuing to collect the rest", file=FSM_PATH))
            event = "done"
        target = next(
            (transition.target for transition in fsm.transitions
             if transition.source == state and transition.event == event),
            None,
        )
        if target is None:
            findings.append(Finding(
                "fsm", Severity.HIGH, "missing-transition",
                f"{state}: no transition for event {event!r} (machine cannot continue)",
                file=FSM_PATH))
            return results, findings, "fail"
        state = target
    return results, findings, state
