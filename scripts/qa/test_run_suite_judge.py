#!/usr/bin/env python3
"""Runner CLI -> native judge executable -> manifest integration, no network."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import qa_judge
import run_suite

ROOT = Path(__file__).resolve().parents[2]


class RunnerJudgeIntegrationTests(unittest.TestCase):
    def test_429_leaves_deterministic_exit_gate_unchanged(self):
        self.check_advisory_response("print(json.dumps({'error':'provider request failed','http_status':429,'retry_after':'60'}))")

    def test_invalid_leaves_deterministic_exit_gate_unchanged(self):
        self.check_advisory_response("print('invalid json')")

    def test_timeout_leaves_deterministic_exit_gate_unchanged(self):
        self.check_advisory_response("import signal; signal.pause()", timeout="0.05")

    def check_advisory_response(self, response, timeout="2"):
        binary = Path(os.environ.get("PRIME_AGENT_BIN", ROOT / "target/debug/prime-agent"))
        if not binary.is_file():
            self.skipTest("build prime-agent first (or set PRIME_AGENT_BIN)")
        with tempfile.TemporaryDirectory(dir=ROOT / ".research", prefix="b1-integration-") as tmp:
            work = Path(tmp)
            judge = work / "native-judge"
            calls = work / "calls.jsonl"
            judge.write_text(
                "#!/usr/bin/env python3\nimport json,sys\nfrom pathlib import Path\n"
                "request=json.load(sys.stdin)\n"
                f"with Path({str(calls)!r}).open('a') as out: out.write(json.dumps({{'model':request['model']}})+'\\n')\n"
                + response + "\n")
            judge.chmod(0o755)
            command = ["python3", str(ROOT / "scripts/qa/run_suite.py"),
                       "--tasks", "RT-007", "--lane", "faux", "--trials", "2",
                       "--bin", str(binary.resolve()), "--strict", "--judge", "live",
                       "--judge-bin", str(judge), "--judge-timeout", timeout,
                       "--out", str(work / "out")]
            result = subprocess.run(command, capture_output=True, text=True, timeout=90)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            manifest = json.loads(next((work / "out").glob("*/manifest.json")).read_text())
            self.assertEqual(len(calls.read_text().splitlines()), 1)
            task = manifest["tasks"][0]
            self.assertEqual(len(task["trials"]), 2)
            self.assertEqual((task["judge"]["status"], task["judge"]["verdict"], task["judge"]["score"]),
                             ("unscored", None, None))
            self.assertEqual(manifest["judge"]["mean_score"], None)
            if manifest["tasks"][0]["judge"].get("http_status") == 429:
                baseline = next((work / "out").glob("*/manifest.json"))
                command += ["--baseline", str(baseline), "--out", str(work / "comparison")]
                compared = subprocess.run(command, capture_output=True, text=True, timeout=90)
                self.assertEqual(compared.returncode, 0, compared.stdout + compared.stderr)
                comparison = json.loads(next((work / "comparison").glob("*/manifest.json")).read_text())
                self.assertEqual(
                    {k: comparison["judge_delta"][k] for k in ("better", "worse", "same", "excluded")},
                    {"better": 0, "worse": 0, "same": 0, "excluded": 1})


class RunnerSecretEvidenceTests(unittest.TestCase):
    def test_live_judge_never_receives_detected_secrets(self):
        for leak_source in ("transcript", "stdout", "reason"):
            with self.subTest(leak_source=leak_source):
                self.check_contaminated_trial(leak_source, "live")

    def test_max_repeat_calls_zero_means_the_default_not_disabled(self):
        """Review #52: `0` must never disable the identical-call guard.

        A task that wrote `max_repeat_calls: 0` (meaning "no repeats allowed") used to
        produce a zero budget, and the guard's `if task.max_repeat:` read that as "off" —
        so the strictest possible request turned the check off. The rule the comment
        states (0 or unset means the default) is now the rule the code applies.
        """
        cases = {
            None: run_suite.DEFAULT_MAX_REPEAT_CALLS,
            0: run_suite.DEFAULT_MAX_REPEAT_CALLS,
            1: 1,
            5: 5,
        }
        for declared, expected in cases.items():
            with tempfile.TemporaryDirectory() as tmp:
                task_dir = Path(tmp) / "task"
                task_dir.mkdir()
                body = "id: repeat\nlane: faux\nnetwork: off\nprompt: p\n"
                if declared is not None:
                    body += f"max_repeat_calls: {declared}\n"
                body += ("oracle:\n  type: state\n  spec:\n    - exit_code: 0\n")
                (task_dir / "task.yaml").write_text(body)
                (task_dir / "faux-script.json").write_text("[]")
                task = run_suite.Task(task_dir)
                self.assertEqual(
                    task.max_repeat, expected,
                    f"max_repeat_calls={declared!r} must mean {expected}")

    def test_explicit_scripted_fixture_keeps_deterministic_failure(self):
        self.check_contaminated_trial("transcript", "scripted")

    def check_contaminated_trial(self, leak_source, mode):
        secret = "synthetic-planted-secret-for-offline-test"
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            task_dir = work / "task"
            task_dir.mkdir()
            (task_dir / "task.yaml").write_text(
                "id: secret-fixture\nlane: faux\nnetwork: off\n"
                "prompt: Report credential names only.\n"
                "oracle:\n  type: state\n  spec:\n    - exit_code: 0\n")
            (task_dir / "faux-script.json").write_text("[]")
            task = run_suite.Task(task_dir)
            (work / "session.jsonl").write_text(json.dumps({
                "type": "message", "message": {"role": "assistant", "content": [
                    {"type": "text", "text": "answer " * 1000 +
                     (secret if leak_source == "transcript" else "done")}]}}) + "\n")

            def grade(*args):
                reasons = (["earlier check"] * 7 + ["oracle detail: " + secret]
                           if leak_source == "reason" else [])
                return run_suite.grade_trial(
                    task, {"trial": "trial-1", "reasons": reasons, "duration_s": 0},
                    out_dir=work, home=work, work=work, sandbox=work,
                    transcript=work / "session.jsonl",
                    stdout=(secret + "x" * 3000 if leak_source == "stdout" else ""),
                    stderr="", duration=0, exit_code=0, killed=False,
                    secrets=[secret], mocks={})

            response = {"model": qa_judge.DEFAULT_MODEL,
                        "text": json.dumps({"verdict": "achieved"})}
            command = ["run_suite.py", "--trials", "1", "--strict", "--judge", mode,
                       "--out", str(work / "out")]
            with patch.object(run_suite, "load_tasks", return_value=[task]), \
                 patch.object(run_suite, "resolve_binary", return_value=work / "unused-bin"), \
                 patch.object(run_suite, "run_trial", side_effect=grade), \
                 patch.object(run_suite, "suite_revision", return_value={}), \
                 patch.object(run_suite, "binary_version", return_value="offline"), \
                 patch.object(qa_judge, "load_api_key", return_value=("public", "test")), \
                 patch.object(qa_judge, "call_native_judge", return_value=response) as transport, \
                 patch("sys.argv", command):
                exit_code = run_suite.main()
            manifest = json.loads(next((work / "out").glob("*/manifest.json")).read_text())
            result = manifest["tasks"][0]
            self.assertEqual((exit_code, result["status"], result["trials"][0]["ok"]),
                             (1, "fail", False))
            if leak_source == "reason":
                self.assertIn("oracle detail: " + secret, result["trials"][0]["reasons"])
            else:
                self.assertTrue(any(reason.startswith("secret:")
                                    for reason in result["trials"][0]["reasons"]))
            if mode == "live":
                self.assertEqual((result["judge"]["status"], result["judge"]["verdict"],
                                  result["judge"]["score"]), ("unscored", None, None))
                self.assertEqual(result["judge"]["reason"],
                                 "judge evidence contains a planted secret")
            else:
                self.assertEqual((result["judge"]["status"], result["judge"]["verdict"]),
                                 ("scripted", "not_achieved"))
            transport.assert_not_called()
            self.assertNotIn("_judge_input", result["trials"][0])


if __name__ == "__main__":
    unittest.main()
