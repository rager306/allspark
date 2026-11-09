#!/usr/bin/env python3
"""Runner CLI -> native judge executable -> manifest integration, no network."""
import contextlib
import io
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


class RunnerFailureExitTests(unittest.TestCase):
    """Review #16: the documented `--lane real-model --tasks ST-001 ST-002` run exits 0
    on a failing task because failures only gated behind the opt-in `--strict`. A
    semantic regression that reads as a green exit is the same defect class as the
    all-skipped run that reported green (review #54): the exit code then claims the
    opposite of what the run observed."""

    def test_a_failing_task_exits_nonzero_without_any_flag(self):
        self.assertEqual(self.run_verbatim(), 1)

    def test_allow_failures_is_the_deliberate_opt_out(self):
        self.assertEqual(self.run_verbatim("--allow-failures"), 0)

    def test_a_baseline_run_keeps_the_delta_gate_as_its_verdict(self):
        """A baseline comparison answers "did this REGRESS", so a known-red task
        must not decide it: a task that is red today is data there, not a failure,
        and the delta gate is the verdict."""
        with tempfile.TemporaryDirectory() as tmp:
            baseline = Path(tmp) / "manifest.json"
            baseline.write_text(json.dumps({
                "tasks": [{"id": "exit-fixture", "pass_all": False,
                           "trials": [{"ok": False}]}]}))
            self.assertEqual(self.run_verbatim("--baseline", str(baseline)), 0)

    def run_verbatim(self, *extra):
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            task_dir = work / "task"
            task_dir.mkdir()
            (task_dir / "task.yaml").write_text(
                "id: exit-fixture\nlane: faux\nnetwork: off\nprompt: p\n"
                "oracle:\n  type: state\n  spec:\n    - exit_code: 0\n")
            (task_dir / "faux-script.json").write_text("[]")
            task = run_suite.Task(task_dir)

            def failing_trial(*args, **kwargs):
                return {"trial": "trial-1", "ok": False, "reasons": ["oracle: exit_code 1"],
                        "duration_s": 0.1, "steps": 0, "tokens": 0, "tool_freq": {},
                        "exit_code": 1, "tool_calls": 0, "killed": False}

            command = ["run_suite.py", "--trials", "1", "--out", str(work / "out"), *extra]
            with patch.object(run_suite, "load_tasks", return_value=[task]), \
                 patch.object(run_suite, "resolve_binary", return_value=work / "unused-bin"), \
                 patch.object(run_suite, "run_trial", side_effect=failing_trial), \
                 patch.object(run_suite, "suite_revision", return_value={}), \
                 patch.object(run_suite, "binary_version", return_value="offline"), \
                 patch("sys.argv", command), \
                 contextlib.redirect_stdout(io.StringIO()):
                return run_suite.main()


class ManifestArtifactTests(unittest.TestCase):
    """Review #21: the manifest is an artifact (kept under --out, archived, and
    uploaded by CI), so a credential must never enter it. `--api-key` was copied into
    `model_cfg` at construction and `model_cfg` was written whole, which put the key in
    plaintext in every real-model manifest."""

    SENTINEL = "qa-sentinel-key-do-not-serialize"

    def test_the_api_key_never_reaches_the_manifest(self):
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            task_dir = work / "task"
            task_dir.mkdir()
            (task_dir / "task.yaml").write_text(
                "id: secret-fixture\nlane: real-model\nnetwork: off\nprompt: p\n"
                "oracle:\n  type: state\n  spec:\n    - exit_code: 0\n")
            task = run_suite.Task(task_dir)

            def passing_trial(*args, **kwargs):
                return {"trial": "trial-1", "ok": True, "reasons": [],
                        "duration_s": 0.1, "steps": 0, "tokens": 0, "tool_freq": {},
                        "exit_code": 0, "tool_calls": 0, "killed": False}

            command = ["run_suite.py", "--trials", "1", "--lane", "real-model",
                       "--provider", "openai", "--model", "gpt-x",
                       "--api-key", self.SENTINEL, "--out", str(work / "out")]
            with patch.object(run_suite, "load_tasks", return_value=[task]), \
                 patch.object(run_suite, "resolve_binary", return_value=work / "unused-bin"), \
                 patch.object(run_suite, "run_trial", side_effect=passing_trial), \
                 patch.object(run_suite, "suite_revision", return_value={}), \
                 patch.object(run_suite, "binary_version", return_value="offline"), \
                 patch("sys.argv", command), \
                 contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(run_suite.main(), 0)

            manifests = list((work / "out").rglob("manifest.json"))
            self.assertEqual(len(manifests), 1, "the run writes one manifest")
            text = manifests[0].read_text()
            self.assertNotIn(self.SENTINEL, text,
                             "the provider API key must never be serialized into an artifact")
            self.assertEqual(json.loads(text)["model"],
                             {"provider": "openai", "model": "gpt-x"},
                             "the manifest keeps the selection, not the credential")


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
                       "--bin", str(binary.resolve()), "--judge", "live",
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
            command = ["run_suite.py", "--trials", "1", "--judge", mode,
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
