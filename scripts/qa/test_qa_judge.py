#!/usr/bin/env python3
"""Offline regression tests for advisory judge failures and score provenance."""

from __future__ import annotations

import json
import subprocess
import unittest
from types import SimpleNamespace
from unittest.mock import patch

import qa_judge


class JudgeRegressionTests(unittest.TestCase):
    def setUp(self):
        self.task = SimpleNamespace(
            id="B1-fixture", title="Offline judge fixture",
            prompt="Write the requested artifact.", judge={},
        )
        self.network = patch("urllib.request.urlopen", side_effect=AssertionError("network forbidden"))
        self.network.start()
        self.addCleanup(self.network.stop)

    def judge(self, **kwargs):
        options = dict(api_key="test-only-key", key_source="test", mode="live",
                       model="test/model", binary="/offline/native-judge")
        options.update(kwargs)
        return qa_judge.judge_task(
            self.task, [], "The requested artifact is complete.", "", [], None,
            **options,
        )

    def assert_unscored(self, result):
        self.assertEqual(
            (result.get("status"), result.get("verdict"), result.get("score")),
            ("unscored", None, None),
        )

    def test_http_429_is_unscored_not_scripted(self):
        response = dict(error="http 429", http_status=429, retry_after="60", duration_ms=1)
        with patch.object(qa_judge, "call_native_judge", return_value=response) as transport:
            result = self.judge()
        self.assert_unscored(result)
        self.assertEqual((result["http_status"], result["retry_after"]), (429, "60"))
        transport.assert_called_once()

    def test_live_without_key_is_unscored_not_scripted(self):
        with patch.object(qa_judge, "call_native_judge") as transport:
            self.assert_unscored(self.judge(api_key=None, key_source="absent"))
        transport.assert_not_called()

    def test_unknown_choice_is_unscored_not_zero(self):
        response = dict(text=json.dumps({"verdict": "unknown"}), model="test/model")
        with patch.object(qa_judge, "call_native_judge", return_value=response):
            self.assert_unscored(self.judge())

    def test_malformed_choices_are_unscored(self):
        for text in ("not json", "[]", "{}", '{"verdict":"unrecognized"}',
                     '{"verdict":["achieved"]}'):
            with self.subTest(text=text):
                response = dict(text=text, model="test/model")
                with patch.object(qa_judge, "call_native_judge", return_value=response):
                    self.assert_unscored(self.judge())

    def test_transport_failures_are_unscored_without_retry(self):
        for failure in (OSError("unavailable"),
                        subprocess.TimeoutExpired("offline-native-judge", 1)):
            with self.subTest(failure=type(failure).__name__):
                with patch.object(qa_judge, "call_native_judge", side_effect=failure) as transport:
                    self.assert_unscored(self.judge())
                transport.assert_called_once()

    def test_scored_zero_preserves_native_measurement_and_usage(self):
        response = dict(text=json.dumps({"verdict": "not_achieved", "reason": "No artifact"}),
                        model="test/model", usage={"input_tokens": 20, "output_tokens": 5},
                        duration_ms=7)
        with patch.object(qa_judge, "call_native_judge", return_value=response) as transport:
            result = self.judge()
        self.assertEqual(
            (result["status"], result["backend"], result["verdict"], result["score"],
             result["usage"], result["duration_ms"]),
            ("scored", "native", "not_achieved", 0, response["usage"], 7),
        )
        request, executable, timeout = transport.call_args.args
        self.assertEqual((request["model"], executable, timeout),
                         ("test/model", "/offline/native-judge", 20.0))

    def test_wrong_response_model_is_unscored(self):
        response = dict(text=json.dumps({"verdict": "achieved"}), model="other/model")
        with patch.object(qa_judge, "call_native_judge", return_value=response):
            self.assert_unscored(self.judge())

    def test_large_evidence_is_capped_and_reports_truncation(self):
        self.task.prompt = "prompt " * 10000
        answer = "answer " * 10000
        outcome = "outcome " * 10000
        telemetry = {"details": "telemetry " * 10000}
        state = qa_judge.build_state(self.task, answer, outcome, [], telemetry)
        self.assertLessEqual(len(state), 12000)
        self.assertIn("[truncated:", state)
        response = dict(text=json.dumps({"verdict": "achieved"}), model="test/model")
        with patch.object(qa_judge, "call_native_judge", return_value=response) as transport:
            result = qa_judge.judge_task(
                self.task, [], answer, outcome, [], telemetry,
                api_key="test-only-key", key_source="test", mode="live", model="test/model",
            )
        self.assertEqual((result["state_chars"], result["state_truncated"]), (len(state), True))
        self.assertEqual(transport.call_args.args[0]["state"], state)

    def test_upstream_evidence_truncation_remains_visible_to_judge(self):
        answer_text = "answer " * 1000
        rows = [{"type": "message", "message": {"role": "assistant", "content": [
            {"type": "text", "text": answer_text}]}}]
        answer = qa_judge.extract_final_answer(rows, report_truncation=True)
        self.assertLessEqual(len(answer), 4000)
        self.assertIn("[truncated:", answer)
        self.assertEqual(qa_judge.extract_final_answer(rows), answer_text[:4000])
        outcome = qa_judge.truncate_evidence("prefix " * 1000 + "final tail", 1200, tail=True)
        self.assertLessEqual(len(outcome), 1200)
        self.assertIn("[truncated:", outcome)
        self.assertTrue(outcome.endswith("final tail"))
        for evidence_answer, evidence_outcome in ((answer, ""), ("done", outcome)):
            with self.subTest(answer_truncated=bool(evidence_answer == answer)):
                state = qa_judge.build_state(self.task, evidence_answer, evidence_outcome, [], None)
                self.assertIn("[truncated:", state)
                response = dict(text=json.dumps({"verdict": "achieved"}), model="test/model")
                with patch.object(qa_judge, "call_native_judge", return_value=response):
                    result = qa_judge.judge_task(
                        self.task, rows, evidence_answer, evidence_outcome, [], None,
                        api_key="public", key_source="test", model="test/model")
                self.assertEqual((result["status"], result["state_truncated"]), ("scored", True))

    def test_scripted_mode_is_explicitly_labelled(self):
        result = self.judge(mode="scripted", api_key=None)
        self.assertEqual((result.get("status"), result.get("backend")), ("scripted", "scripted"))


class JudgeComparisonTests(unittest.TestCase):
    def row(self, verdict="achieved", **changes):
        judge = dict(
            status="scored", backend="native", bundle="test-bundle", model="test/model",
            prompt_rev="test-rev", rubric_sha256="test-rubric", verdict=verdict,
            score=qa_judge.verdict_rank(verdict),
        )
        judge.update(changes)
        return {"id": "B1-fixture", "judge": judge}

    def test_matching_provenance_compares_zero_against_positive(self):
        delta = qa_judge.compare_judge_deltas([self.row()], [self.row("not_achieved")])
        self.assertEqual(
            {key: delta[key] for key in ("pairs", "better", "worse", "same", "excluded")},
            dict(pairs=1, better=1, worse=0, same=0, excluded=0),
        )

    def test_each_provenance_mismatch_is_excluded(self):
        for field in ("backend", "bundle", "model", "prompt_rev", "rubric_sha256"):
            with self.subTest(field=field):
                delta = qa_judge.compare_judge_deltas(
                    [self.row(**{field: "different"})], [self.row("not_achieved")],
                )
                self.assertEqual(
                    {key: delta[key] for key in ("better", "worse", "same", "excluded")},
                    dict(better=0, worse=0, same=0, excluded=1),
                )

    def test_unknown_is_not_zero(self):
        self.assertIsNone(qa_judge.verdict_rank("unknown"))
        self.assertEqual(qa_judge.verdict_rank("not_achieved"), 0)
        delta = qa_judge.compare_judge_deltas([self.row("unknown")], [self.row("not_achieved")])
        self.assertEqual((delta["excluded"], delta["worse"]), (1, 0))

    def test_error_status_cannot_supply_a_rankable_verdict(self):
        delta = qa_judge.compare_judge_deltas(
            [self.row("not_achieved", status="unscored", score=None)], [self.row()],
        )
        self.assertEqual((delta["excluded"], delta["worse"]), (1, 0))

    def test_missing_provenance_is_excluded(self):
        for field in ("backend", "bundle", "model", "prompt_rev", "rubric_sha256"):
            with self.subTest(field=field):
                current = self.row()
                del current["judge"][field]
                delta = qa_judge.compare_judge_deltas([current], [self.row("not_achieved")])
                self.assertEqual((delta["excluded"], delta["better"]), (1, 0))

    def test_numeric_summary_excludes_none_and_scripted_but_includes_zero(self):
        summary = qa_judge.summarize_judges([
            self.row("not_achieved"), self.row("achieved"),
            self.row(None, status="unscored", score=None),
            self.row("achieved", status="scripted", backend="scripted"),
        ])
        self.assertEqual((summary["scored"], summary["mean_score"]), (2, 1.0))

    def test_numeric_summary_with_no_scores_is_not_zero(self):
        for rows in ([], [self.row(None, status="unscored", score=None)]):
            with self.subTest(rows=rows):
                summary = qa_judge.summarize_judges(rows)
                self.assertEqual((summary["scored"], summary["mean_score"]), (0, None))


class NativeJudgeRequestTests(unittest.TestCase):
    def test_request_auth_uses_stdin_not_arguments(self):
        request = dict(api_key="test-only-secret", model="test/model", state="evidence")
        response = dict(text='{"verdict":"partial"}', model="test/model", usage={}, duration_ms=1)
        completed = subprocess.CompletedProcess(["/offline/native-judge"], 0,
                                                json.dumps(response), "")
        with patch.object(qa_judge.subprocess, "run", return_value=completed) as run:
            result = qa_judge.call_native_judge(request, "/offline/native-judge", 3.0)
        self.assertEqual(result, response)
        self.assertEqual(run.call_args.args, (["/offline/native-judge"],))
        self.assertEqual(json.loads(run.call_args.kwargs["input"]), request)


if __name__ == "__main__":
    unittest.main()
