#!/usr/bin/env python3
"""The runner's `must:` rules grade a tool that ANSWERED, not one that was tried.

Review #37: `must: {tool: graph, args_regex: ...}` counted a call from the
transcript whether or not its result arrived or whether that result was an error. A
task whose whole point is "the graph answered this" therefore passed on a failed
call. `check_invariants` now requires a matching call whose `toolResult` exists and
carries no error; this pins that against the runner's own trace shape.
"""
from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

import run_suite


def task_yaml(must: str) -> str:
    return ("id: must-fixture\nlane: faux\nnetwork: off\nprompt: p\n"
            "oracle:\n  type: state\n  spec:\n    - exit_code: 0\n"
            f"must:\n{must}")


class MustRuleSuccessTests(unittest.TestCase):
    def task(self, must: str) -> run_suite.Task:
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        path = Path(tmp.name)
        (path / "task.yaml").write_text(task_yaml(must))
        return run_suite.Task(path)

    def trace(self, call_id: str, *, result: str | None) -> dict:
        rows = [{"type": "message", "message": {"role": "assistant", "content": [
            {"type": "toolCall", "id": call_id, "name": "graph",
             "arguments": {"action": "trace"}}]}}]
        if result is not None:
            rows.append({"type": "message", "message": {
                "role": "toolResult", "toolCallId": call_id, "toolName": "graph",
                "content": [{"type": "text", "text": "main -> handle"}],
                "isError": result == "error"}})
        transcript = Path(tempfile.mkdtemp()) / "session.jsonl"
        self.addCleanup(lambda: transcript.parent.exists() and None)
        transcript.write_text("\n".join(json.dumps(row) for row in rows) + "\n")
        return run_suite.parse_transcript(transcript)

    def problems(self, must: str, trace: dict) -> list[str]:
        return run_suite.check_invariants(
            self.task(must), trace, "", "", 1.0, False, [], "off")

    def test_a_call_whose_result_succeeded_satisfies_the_rule(self):
        rule = '  - tool: graph\n    args_regex: "trace"\n'
        self.assertEqual(self.problems(rule, self.trace("c1", result="ok")), [])

    def test_a_call_whose_result_errored_does_not(self):
        rule = '  - tool: graph\n    args_regex: "trace"\n'
        problems = self.problems(rule, self.trace("c1", result="error"))
        self.assertEqual(len(problems), 1)
        self.assertIn("failed or returned no result", problems[0])

    def test_a_call_with_no_result_row_does_not(self):
        rule = '  - tool: graph\n    args_regex: "trace"\n'
        problems = self.problems(rule, self.trace("c1", result=None))
        self.assertEqual(len(problems), 1)
        self.assertIn("failed or returned no result", problems[0])

    def test_a_call_the_regex_misses_is_still_reported_as_absent(self):
        rule = '  - tool: graph\n    args_regex: "search"\n'
        problems = self.problems(rule, self.trace("c1", result="ok"))
        self.assertEqual(len(problems), 1)
        self.assertIn("no call matching", problems[0])


if __name__ == "__main__":
    unittest.main()
