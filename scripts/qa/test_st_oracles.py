#!/usr/bin/env python3
"""The ST-* oracles must separate a WRONG answer from a RIGHT one (review #36-#39).

The four semantic tasks ship an oracle each, and each oracle had the same defect
class: it graded a property of the FILE or the TRANSCRIPT, not of the answer, so a
wrong answer passed. Measured, not imagined:

* ST-003: leaving `describe` alone, corrupting `summary`, and putting the marker in a
  COMMENT passed the file-wide oracle (exit 0);
* ST-002: the `CHAIN-DONE` marker was searched over the whole transcript, and the
  task's own prompt carries it, so the marker check was always satisfied;
* ST-001: `spawn_worker` / `unrelated` anywhere in the answer passed a SWAPPED
  mapping;
* ST-004: `DELETE` always passed, whether or not jev was called or disagreed.

So each test here runs the shipped oracle twice - once on a deliberately wrong
outcome, once on the correct one - and requires red then green. An oracle that
cannot fail is the same defect as a guard that cannot fail (doc/agent-errors.md).
Run: `python3 -m unittest discover -s scripts/qa -p 'test_*.py'`.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TASKS = ROOT / "qa-tasks"


def run_oracle(task_id: str, work: Path, transcript: Path | None = None) -> tuple[int, str]:
    env = dict(os.environ)
    env["QA_WORK"] = str(work)
    env["QA_TRANSCRIPT"] = str(transcript) if transcript else ""
    env["QA_SCRIPTS_DIR"] = str(ROOT / "scripts" / "qa")
    env["QA_HOME"] = str(work)
    proc = subprocess.run(
        [sys.executable, str(TASKS / task_id / "oracle.py")],
        capture_output=True, text=True, env=env, timeout=60,
    )
    return proc.returncode, (proc.stdout + proc.stderr).strip()


def transcript(path: Path, rows: list[dict]) -> Path:
    path.write_text("\n".join(json.dumps(row) for row in rows) + "\n", encoding="utf-8")
    return path


def assistant(text: str) -> dict:
    return {"type": "message",
            "message": {"role": "assistant", "content": [{"type": "text", "text": text}]}}


def tool_call(call_id: str, name: str, arguments: dict) -> dict:
    return {"type": "message", "message": {"role": "assistant", "content": [
        {"type": "toolCall", "id": call_id, "name": name, "arguments": arguments}]}}


def tool_result(call_id: str, name: str, text: str, *, details: dict | None = None,
                error: bool = False) -> dict:
    return {"type": "message", "message": {
        "role": "toolResult", "toolCallId": call_id, "toolName": name,
        "content": [{"type": "text", "text": text}],
        "details": details or {}, "isError": error}}


class Fixture:
    """A scratch copy of one task's fixture, plus a transcript slot."""

    def __init__(self, task_id: str):
        self.tmp = tempfile.TemporaryDirectory()
        self.work = Path(self.tmp.name) / "work"
        shutil.copytree(TASKS / task_id / "fixture", self.work)
        self.transcript_path = Path(self.tmp.name) / "session.jsonl"

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.tmp.cleanup()

    def edit(self, relative: str, old: str, new: str) -> None:
        path = self.work / relative
        text = path.read_text(encoding="utf-8")
        assert old in text, f"{relative} does not contain {old!r}"
        path.write_text(text.replace(old, new, 1), encoding="utf-8")

    def run(self, task_id: str, rows: list[dict] | None = None) -> tuple[int, str]:
        path = transcript(self.transcript_path, rows or [])
        return run_oracle(task_id, self.work, path)


class SymbolNavigationOracleTests(unittest.TestCase):
    """ST-001: the mapping is graded line by line, and the edit by method body."""

    task = "ST-001-symbol-nav-not-grep"
    original_worker = "        self.id * 2"
    new_worker = "        self.id * 10"

    def correct(self, fx: Fixture, answer: str) -> tuple[int, str]:
        fx.edit("src/lib.rs", self.original_worker, self.new_worker)
        return fx.run(self.task, [assistant(answer)])

    def test_the_correct_mapping_and_edit_pass(self):
        answer = "Worker::run <- spawn_worker\nBackup::run <- unrelated\nNAV-DONE"
        with Fixture(self.task) as fx:
            code, output = self.correct(fx, answer)
        self.assertEqual((code, output), (0, ""))

    def test_a_swapped_mapping_fails(self):
        answer = "Worker::run <- unrelated\nBackup::run <- spawn_worker\nNAV-DONE"
        with Fixture(self.task) as fx:
            code, output = self.correct(fx, answer)
        self.assertEqual(code, 1)
        self.assertIn("Worker::run's caller is reported as 'unrelated'", output)

    def test_naming_both_functions_in_prose_is_not_a_mapping(self):
        answer = "worker.run() is called from spawn_worker, backup.run() from unrelated. NAV-DONE"
        with Fixture(self.task) as fx:
            code, output = self.correct(fx, answer)
        self.assertEqual(code, 1)
        self.assertIn("does not report both call sites", output)

    def test_the_marker_landing_in_the_backup_body_fails(self):
        answer = "Worker::run <- spawn_worker\nBackup::run <- unrelated\nNAV-DONE"
        with Fixture(self.task) as fx:
            fx.edit("src/lib.rs", self.original_worker, self.new_worker)
            backup = "        self.id * 3"
            fx.edit("src/lib.rs", backup, "        self.id * 10")
            code, output = fx.run(self.task, [assistant(answer)])
        self.assertEqual(code, 1)
        self.assertIn("Backup::run was modified", output)


class ArchitectureOracleTests(unittest.TestCase):
    """ST-002: a SUCCESSFUL graph call, and the chain in the FINAL answer."""

    task = "ST-002-architecture-via-graph"
    chain = "main -> handle -> load -> validate -> 42\nCHAIN-DONE"

    def rows(self, *, result: bool = True, error: bool = False,
             answer: str | None = None) -> list[dict]:
        rows = [tool_call("c1", "graph", {"action": "trace"})]
        if result:
            rows.append(tool_result("c1", "graph", "main -> handle -> load", error=error))
        rows.append(assistant(self.chain if answer is None else answer))
        return rows

    def test_a_successful_graph_call_with_the_right_order_passes(self):
        with Fixture(self.task) as fx:
            code, output = fx.run(self.task, self.rows())
        self.assertEqual((code, output), (0, ""))

    def test_a_graph_call_that_failed_is_not_a_graph_call(self):
        with Fixture(self.task) as fx:
            code, output = fx.run(self.task, self.rows(error=True))
        self.assertEqual(code, 1)
        self.assertIn("every graph call failed", output)

    def test_a_graph_call_with_no_result_is_not_a_graph_call(self):
        with Fixture(self.task) as fx:
            code, output = fx.run(self.task, self.rows(result=False))
        self.assertEqual(code, 1)
        self.assertIn("every graph call failed", output)

    def test_the_wrong_evaluation_order_fails(self):
        with Fixture(self.task) as fx:
            code, output = fx.run(
                self.task, self.rows(answer="main -> handle -> validate -> load\nCHAIN-DONE"))
        self.assertEqual(code, 1)
        self.assertIn("orders `validate` before `load`", output)

    def test_the_marker_must_be_in_the_answer_not_just_the_prompt(self):
        with Fixture(self.task) as fx:
            code, output = fx.run(
                self.task, self.rows(answer="main -> handle -> load -> validate"))
        self.assertEqual(code, 1)
        self.assertIn("missing CHAIN-DONE", output)


class SymbolEditOracleTests(unittest.TestCase):
    """ST-003: each method body is checked, so a comment cannot satisfy the edit."""

    task = "ST-003-edit-symbol-body-not-sed"
    original = '        format!("retries={} timeout={}", self.retries, self.timeout_ms)'
    updated = '        format!("describe: retries={} timeout={}", self.retries, self.timeout_ms)'
    describe_doc = "    /// Replaced wholesale by the task."

    def test_the_right_body_change_passes(self):
        with Fixture(self.task) as fx:
            fx.edit("src/lib.rs", self.original, self.updated)
            code, output = fx.run(self.task)
        self.assertEqual((code, output), (0, ""))

    def test_the_marker_in_a_comment_with_summary_corrupted_fails(self):
        """The exact wrong answer that the file-wide oracle accepted (review #38)."""
        with Fixture(self.task) as fx:
            fx.edit("src/lib.rs", self.describe_doc, self.describe_doc + " describe: retries=")
            fx.edit("src/lib.rs", '        format!("retries={} timeout={}", self.retries, self.timeout_ms)\n    }\n}',
                    '        format!("BROKEN", self.retries, self.timeout_ms)\n    }\n}')
            code, output = fx.run(self.task)
        self.assertEqual(code, 1)
        self.assertIn("Config::describe does not return", output)
        self.assertIn("Config::summary's body is not byte-identical", output)

    def test_an_unmodified_fixture_fails(self):
        with Fixture(self.task) as fx:
            code, output = fx.run(self.task)
        self.assertEqual(code, 1)
        self.assertIn("Config::describe does not return", output)


class TypedDecisionOracleTests(unittest.TestCase):
    """ST-004: decision.txt must agree with the probability jev returned."""

    task = "ST-004-typed-decision-not-guess"

    def rows(self, noul: float | None, *, call: bool = True, details: bool = True,
             error: bool = False) -> list[dict]:
        rows = []
        if call:
            rows.append(tool_call("j1", "jev", {"action": "check", "question": "delete?",
                                                 "state": "README"}))
            rows.append(tool_result(
                "j1", "jev",
                f"check: {noul:.2f}" if noul is not None else "check: failed",
                details={"answers": {"check": {"type": "noul", "noul": noul}}} if details and noul is not None else {},
                error=error))
        return rows

    def decide(self, decision: str, rows: list[dict]) -> tuple[int, str]:
        with Fixture(self.task) as fx:
            (fx.work / "decision.txt").write_text(decision + "\n", encoding="utf-8")
            return fx.run(self.task, rows)

    def test_delete_agreeing_with_a_high_probability_passes(self):
        self.assertEqual(self.decide("DELETE", self.rows(0.9)), (0, ""))

    def test_keep_agreeing_with_a_low_probability_passes(self):
        self.assertEqual(self.decide("KEEP", self.rows(0.1)), (0, ""))

    def test_delete_against_a_low_probability_fails(self):
        code, output = self.decide("DELETE", self.rows(0.1))
        self.assertEqual(code, 1)
        self.assertIn("jev answered p(yes)=0.10 (KEEP)", output)

    def test_a_decision_without_a_jev_call_fails(self):
        code, output = self.decide("DELETE", self.rows(None, call=False))
        self.assertEqual(code, 1)
        self.assertIn("no successful jev call", output)

    def test_a_failed_jev_call_is_not_a_verdict(self):
        code, output = self.decide("DELETE", self.rows(0.9, error=True))
        self.assertEqual(code, 1)
        self.assertIn("no successful jev call", output)

    def test_the_rendered_text_is_read_when_details_are_absent(self):
        self.assertEqual(self.decide("DELETE", self.rows(0.9, details=False)), (0, ""))


if __name__ == "__main__":
    unittest.main()
