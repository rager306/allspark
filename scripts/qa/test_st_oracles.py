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
import re
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

    def test_a_marker_in_a_dead_branch_does_not_satisfy_the_return(self):
        """Review #36's residual: the marker must be in what `run` RETURNS.

        `if false { return self.id * 10; }` beside a live `self.id * 4` kept the old
        presence/absence pair green while the method returned the wrong number.
        """
        answer = "Worker::run <- spawn_worker\nBackup::run <- unrelated\nNAV-DONE"
        with Fixture(self.task) as fx:
            fx.edit(
                "src/lib.rs",
                self.original_worker,
                "    pub fn run(&self) -> u32 {\n"
                "        if false { return self.id * 10; }\n"
                "        self.id * 4\n"
                "    }",
            )
            code, output = fx.run(self.task, [assistant(answer)])
        self.assertEqual(code, 1)
        self.assertIn("does not RETURN", output)

    def test_the_marker_landing_in_the_backup_body_fails(self):
        answer = "Worker::run <- spawn_worker\nBackup::run <- unrelated\nNAV-DONE"
        with Fixture(self.task) as fx:
            fx.edit("src/lib.rs", self.original_worker, self.new_worker)
            backup = "        self.id * 3"
            fx.edit("src/lib.rs", backup, "        self.id * 10")
            code, output = fx.run(self.task, [assistant(answer)])
        self.assertEqual(code, 1)
        # The claim is that Backup's RETURNED value changed (review #36: a body-wide
        # presence check would miss a marker moved into a dead branch).
        self.assertIn("Backup::run's RETURNED value changed", output)


class FixturePremiseTests(unittest.TestCase):
    """ST-001's fixture must be a trap: the two call sites cannot disclose the mapping.

    Review #36: the prompt claimed both call sites read `x.run()` while the fixture spelled
    `worker.run()` / `backup.run()`, so a text search mapped each receiver to its type and the
    "symbol-aware lookup" the task exists to test was never needed. These pin the fixture, and
    the prompt, against drifting apart again.
    """

    task = "ST-001-symbol-nav-not-grep"

    def source(self) -> str:
        return (TASKS / self.task / "fixture" / "src" / "lib.rs").read_text(encoding="utf-8")

    def test_the_two_call_sites_are_textually_identical(self):
        sites = [line.strip() for line in self.source().splitlines() if ".run()" in line]
        self.assertEqual(len(sites), 2, f"expected exactly two call sites: {sites}")
        self.assertEqual(
            sites[0], sites[1],
            "the call sites differ, so a text search maps each receiver to its type "
            f"without the symbol layer: {sites}")

    def test_the_prompt_quotes_the_call_site_the_fixture_has(self):
        prompt = (TASKS / self.task / "task.yaml").read_text(encoding="utf-8")
        match = re.search(r"call sites? read `([^`]+)`", prompt)
        self.assertIsNotNone(match, "the prompt no longer states the call-site form it traps")
        form = match.group(1)
        occurrences = [line.strip() for line in self.source().splitlines() if line.strip() == form]
        self.assertEqual(
            len(occurrences), 2,
            f"the prompt promises {form!r} at both sites; the fixture has {occurrences}")


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

    def test_a_marker_in_a_comment_inside_the_describe_body_fails(self):
        """Review #38's residual case: the marker is in describe's CODE-adjacent comment.

        The body-wide `in` check cannot tell a comment from a statement, so a body that
        carries `// describe: retries=` and returns something else satisfied it.
        """
        with Fixture(self.task) as fx:
            fx.edit("src/lib.rs", self.describe_doc, self.describe_doc + " describe: retries=")
            fx.edit(
                "src/lib.rs",
                '        format!("retries={} timeout={}", self.retries, self.timeout_ms)\n    }',
                '        // describe: retries= and timeout= are gone\n'
                '        String::from("nope")\n    }',
            )
            code, output = fx.run(self.task)
        self.assertEqual(code, 1)
        self.assertIn("Config::describe does not return", output)

    def test_the_right_literal_with_swapped_arguments_fails(self):
        """The prefix inside a literal is not enough (review #38, residual).

        `format!("describe: retries={} timeout={}", self.timeout_ms, self.retries)` prints
        the two fields in the wrong order while every earlier check passes: the marker is in
        a string literal, the original body is gone, and summary is untouched. The fixture's
        `retries` and `timeout_ms` differ, so the order is observable.
        """
        with Fixture(self.task) as fx:
            fx.edit(
                "src/lib.rs",
                self.original,
                '        format!("describe: retries={} timeout={}", self.timeout_ms, self.retries)',
            )
            code, output = fx.run(self.task)
        self.assertEqual(code, 1)
        self.assertIn("wrong order", output)

    def test_a_marker_in_an_unused_binding_does_not_satisfy_the_return(self):
        """Review #38's residual: the marker must be in what the body RETURNS.

        `let _unused = "describe: retries= …"; String::from("nope")` puts the marker in a
        string literal inside the body and returns something else entirely — the earlier
        check ("a literal in the body carries the marker") passed it.
        """
        with Fixture(self.task) as fx:
            fx.edit(
                "src/lib.rs",
                self.original,
                '        let _unused = "describe: retries= timeout=";\n'
                '        String::from("nope")',
            )
            code, output = fx.run(self.task)
        self.assertEqual(code, 1)
        self.assertIn("RETURNED text does not carry", output)

    def test_an_unmodified_fixture_fails(self):
        with Fixture(self.task) as fx:
            code, output = fx.run(self.task)
        self.assertEqual(code, 1)
        self.assertIn("Config::describe does not return", output)


class ArchitectureChainTests(unittest.TestCase):
    """ST-002's chain: every hop in evaluation order, not four names anywhere.

    Review #37's residual: the old check only asked whether each of four names appeared
    somewhere in the answer, so `validate -> load -> handle -> main` (source order,
    reversed) passed. The answer is graded as the ordered list the prompt asks for, and a
    hop that is missing or misplaced fails.
    """

    task = "ST-002-architecture-via-graph"

    def verdict(self, answer: str) -> tuple[int, str]:
        rows = [tool_call("c1", "graph", {"action": "trace"}),
                tool_result("c1", "graph", "main -> handle -> load"),
                assistant(answer)]
        with Fixture(self.task) as fx:
            return fx.run(self.task, rows)

    def test_the_reversed_chain_fails(self):
        code, output = self.verdict("validate -> load -> handle -> main\nCHAIN-DONE")
        self.assertEqual(code, 1, output)

    def test_a_chain_that_omits_a_hop_fails(self):
        code, output = self.verdict("main -> load -> validate\nCHAIN-DONE")
        self.assertEqual(code, 1)
        self.assertIn("handle", output)

    def test_a_chain_ending_at_the_value_the_prompt_names_passes(self):
        """The prompt asks for the chain to the value 41, so stopping there is correct."""
        self.assertEqual(self.verdict("main -> handle -> load -> 41\nCHAIN-DONE"), (0, ""))
        self.assertEqual(self.verdict("main -> handle -> load -> validate -> 42\nCHAIN-DONE"),
                         (0, ""))

    def test_a_chain_with_invented_hops_fails(self):
        """Every required hop in order is not the chain the CODE has (review #37, residual).

        `main -> bogus -> handle -> nonsense -> load -> 41` names main, handle and load in
        evaluation order, so the sequence check alone accepted it while the agent reported
        functions the fixture never contained.
        """
        code, output = self.verdict(
            "main -> bogus -> handle -> nonsense -> load -> 41\nCHAIN-DONE")
        self.assertEqual(code, 1, "an invented hop is not the fixture's chain")
        self.assertIn("bogus", output)

    def test_a_chain_ending_at_a_value_the_fixture_never_produces_fails(self):
        """Only 41 (load) and 42 (validate) exist; `-> 999` used to pass (review #37)."""
        code, output = self.verdict("main -> handle -> load -> 999\nCHAIN-DONE")
        self.assertEqual(code, 1)
        self.assertIn("999", output)

    def test_a_value_cannot_precede_the_call_that_produces_it(self):
        code, output = self.verdict("main -> 41 -> handle -> load -> validate\nCHAIN-DONE")
        self.assertEqual(code, 1, "41 is a value, not a call: the chain is main -> handle -> ...")
        self.assertIn("value before", output)

    def test_a_value_without_its_producer_fails(self):
        """`load -> 42` names a value only `validate` returns, with no `validate` (review #37)."""
        code, output = self.verdict("main -> handle -> load -> 42\nCHAIN-DONE")
        self.assertEqual(code, 1)
        self.assertIn("names no `validate` call", output)

    def test_prose_naming_every_function_is_not_a_chain(self):
        code, output = self.verdict(
            "main calls handle, which calls load and then validate. CHAIN-DONE")
        self.assertEqual(code, 1)
        self.assertIn("ordered", output)


class TypedDecisionOracleTests(unittest.TestCase):
    """ST-004: decision.txt must agree with the probability jev returned."""

    task = "ST-004-typed-decision-not-guess"

    def rows(self, noul: float | None, *, call: bool = True, details: bool = True,
             error: bool = False) -> list[dict]:
        rows = []
        if call:
            # The question names the SERVICE (review #39): a deletion stem alone is not the
            # task's question, and `delete?` used to license any verdict.
            rows.append(tool_call("j1", "jev", {"action": "check",
                                                 "question": "delete this legacy service?",
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

    def test_a_deletion_stem_on_a_non_service_question_is_not_the_task_question(self):
        """`should we delete the temp cache?` carries `delet` and is not about the service."""
        rows = [tool_call("j1", "jev", {"action": "check",
                                        "question": "should we delete the temp cache?"})]
        rows.append(tool_result(
            "j1", "jev", "check: 0.90",
            details={"answers": {"check": {"type": "noul", "noul": 0.9}}}))
        code, output = self.decide("DELETE", rows)
        self.assertEqual(code, 1)
        self.assertIn("never put to the tool", output)

    def test_a_question_about_a_fixture_file_is_not_about_the_service(self):
        """`should we delete the README?` carries `delet` too (review #39, residual).

        The stem rule alone licensed DELETE from a question about a file: deleting a file
        is not deleting the service, so the answer cannot decide the task.
        """
        rows = [tool_call("j1", "jev", {"action": "check", "question": "delete the README?",
                                        "state": "README"})]
        rows.append(tool_result(
            "j1", "jev", "check: 0.90",
            details={"answers": {"check": {"type": "noul", "noul": 0.9}}}))
        code, output = self.decide("DELETE", rows)
        self.assertEqual(code, 1, "a file question is not the service question")
        self.assertIn("never put to the tool", output)

    def test_two_answers_that_license_different_decisions_fail(self):
        """`verdicts[-1]` let an agent ask twice and act on whichever answer suited it."""
        rows = [tool_call("j1", "jev", {"action": "check",
                                         "question": "delete this legacy service?", "state": "s"}),
                tool_call("j2", "jev", {"action": "check",
                                         "question": "delete this legacy service?", "state": "s"})]
        rows.append(tool_result("j1", "jev", "check: 0.10",
                                details={"answers": {"check": {"type": "noul", "noul": 0.1}}}))
        rows.append(tool_result("j2", "jev", "check: 0.90",
                                details={"answers": {"check": {"type": "noul", "noul": 0.9}}}))
        code, output = self.decide("DELETE", rows)
        self.assertEqual(code, 1)
        self.assertIn("different decisions", output)

    def test_the_rendered_text_is_read_when_details_are_absent(self):
        self.assertEqual(self.decide("DELETE", self.rows(0.9, details=False)), (0, ""))

    def test_an_unrelated_jev_question_is_not_the_requested_verdict(self):
        """Review #39's residual: the probability came from ANY successful jev call.

        A `check` about something else (here: whether the README is long) is not the
        task's question, so its 0.9 must not license a DELETE.
        """
        rows = [
            tool_call("j1", "jev", {"action": "check", "question": "is the README long?",
                                     "state": "README"}),
            tool_result("j1", "jev", "check: 0.90",
                        details={"answers": {"check": {"type": "noul", "noul": 0.9}}}),
        ]
        code, output = self.decide("DELETE", rows)
        self.assertEqual(code, 1)
        self.assertIn("no jev call asked", output)

    def test_an_inverted_question_flips_the_word_it_licenses(self):
        """`should we keep it?` at 0.90 licenses KEEP, not DELETE.

        The stems name the direction; p(yes) agreeing with it is the decision. Comparing
        `p >= 0.5` straight to DELETE would invert an inverted question.
        """
        rows = [
            tool_call("j1", "jev", {"action": "check",
                                     "question": "should we keep this legacy service?",
                                     "state": "README"}),
            tool_result("j1", "jev", "check: 0.90",
                        details={"answers": {"check": {"type": "noul", "noul": 0.9}}}),
        ]
        self.assertEqual(self.decide("KEEP", rows), (0, ""))
        code, _ = self.decide("DELETE", rows)
        self.assertEqual(code, 1)

    def test_the_answer_is_read_under_the_asks_own_id(self):
        """The verdict is the answer for the id the call asked, not the first in the map.

        The stray answer sorts first and says 0.90; the requested question's own answer
        says 0.10, so the honest decision is KEEP.
        """
        rows = [
            tool_call("j1", "jev", {"action": "ask", "state": "README", "questions": [
                {"id": "should-delete", "type": "noul",
                 "instructions": "should this legacy service be deleted now?"}]}),
            tool_result("j1", "jev", "aaa: 0.90\nshould-delete: 0.10",
                        details={"answers": {
                            "aaa": {"type": "noul", "noul": 0.9},
                            "should-delete": {"type": "noul", "noul": 0.1},
                        }}),
        ]
        code, output = self.decide("DELETE", rows)
        self.assertEqual(code, 1)
        self.assertIn("0.10", output)
        self.assertEqual(self.decide("KEEP", rows), (0, ""))


if __name__ == "__main__":
    unittest.main()
