"""Unit tests for the interpreter itself: a machine that cannot fail proves nothing."""

from __future__ import annotations

from pathlib import Path

import pytest
import yaml

from testgov.fsm import ALLOWED_KINDS, Fsm, Transition, load_fsm, validate, walk
from testgov.protocol import CheckResult, Context, Severity


def _write(tmp_path: Path, body: dict) -> Path:
    path = tmp_path / "doc" / "testing" / "fsm.yaml"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(yaml.safe_dump(body))
    return tmp_path


GOOD = {
    "initial": "idle",
    "terminal": ["ok", "fail"],
    "states": {
        "idle": {"kind": "noop"},
        "decide": {"kind": "decide"},
    },
    "transitions": [
        {"from": "idle", "event": "done", "to": "decide"},
        {"from": "decide", "event": "ok", "to": "ok"},
        {"from": "decide", "event": "fail", "to": "fail"},
    ],
}


def test_a_well_formed_machine_validates(tmp_path):
    fsm = load_fsm(_write(tmp_path, GOOD))
    assert fsm.ok
    assert validate(fsm) == []


def test_an_unknown_kind_is_a_finding(tmp_path):
    body = {**GOOD, "states": {**GOOD["states"], "idle": {"kind": "teleport"}}}
    fsm = load_fsm(_write(tmp_path, body))
    categories = [finding.category for finding in validate(fsm)]
    assert "unknown-fsm-kind" in categories


def test_a_state_with_no_outgoing_transition_is_a_finding(tmp_path):
    body = {**GOOD, "transitions": GOOD["transitions"][1:]}
    fsm = load_fsm(_write(tmp_path, body))
    categories = [finding.category for finding in validate(fsm)]
    assert "missing-transition" in categories


def test_a_machine_that_cannot_reach_fail_is_a_finding(tmp_path):
    body = {**GOOD, "transitions": [
        {"from": "idle", "event": "done", "to": "decide"},
        {"from": "decide", "event": "ok", "to": "ok"},
        {"from": "decide", "event": "fail", "to": "ok"},
    ]}
    fsm = load_fsm(_write(tmp_path, body))
    categories = [finding.category for finding in validate(fsm)]
    assert "unreachable-fail" in categories


def test_a_missing_transition_at_runtime_stops_the_walk(tmp_path):
    """`decide` emits `fail` here (a selftest problem), and no `fail` transition exists."""
    body = {
        "initial": "idle",
        "terminal": ["ok", "fail"],
        "states": {
            "idle": {"kind": "noop"},
            "verify": {"kind": "run_selftest"},
            "decide": {"kind": "decide"},
        },
        "transitions": [
            {"from": "idle", "event": "done", "to": "verify"},
            {"from": "verify", "event": "done", "to": "decide"},
            {"from": "verify", "event": "fail", "to": "decide"},
            {"from": "decide", "event": "ok", "to": "ok"},
        ],
    }
    fsm = load_fsm(_write(tmp_path, body))
    _results, findings, terminal = walk(
        fsm, Context(root=tmp_path), {}, lambda _ctx: ["a check cannot fail"],
    )
    assert terminal == "fail"
    assert any(finding.category == "missing-transition" for finding in findings)


def test_an_unknown_check_name_is_a_finding(tmp_path):
    body = {
        "initial": "run",
        "terminal": ["ok", "fail"],
        "states": {"run": {"kind": "run_checks", "checks": ["no-such-check"]}, "decide": {"kind": "decide"}},
        "transitions": [
            {"from": "run", "event": "done", "to": "decide"},
            {"from": "run", "event": "fail", "to": "fail"},
            {"from": "decide", "event": "ok", "to": "ok"},
            {"from": "decide", "event": "fail", "to": "fail"},
        ],
    }
    fsm = load_fsm(_write(tmp_path, body))
    _results, findings, terminal = walk(fsm, Context(root=tmp_path), {}, lambda _ctx: [])
    assert terminal == "fail"
    assert any(finding.category == "unknown-check" for finding in findings)


def test_the_shipped_machine_validates_and_reaches_a_terminal_state():
    root = Path(__file__).resolve().parents[2]
    fsm = load_fsm(root)
    assert fsm.ok, fsm.error
    assert validate(fsm) == [], [finding.format() for finding in validate(fsm)]
    assert ALLOWED_KINDS >= {spec.get("kind") for spec in fsm.states.values()}
