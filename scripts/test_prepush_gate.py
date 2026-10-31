#!/usr/bin/env python3
"""The prepush gate must be RUNNABLE, and its guards must be able to fire.

Why this file exists: `317cfc4a1` replaced `ROOT="$PWD"` with a bare `export ROOT`,
so `$ROOT` was unbound, `set -u` aborted on the next line, and the gate did not run
again for three days. Nothing noticed — a script that exits 1 with `unbound variable`
looks like a red gate, and red gates are the normal state of a busy tree.

So this is the smallest test that can fail for that defect: execute the script and
require it to get PAST its own prologue. `--fast` is used because the full gate
compiles for minutes and this test is about reachability, not about the suite.
"""
import pathlib
import shutil
import subprocess
import tempfile
import unittest

GATE = pathlib.Path(__file__).resolve().parent / "fork" / "prepush-gate.sh"


def gate_path():
    """A PATH that can actually run the gate's prologue.

    The prologue test builds its own environment, and the gate's fork-surface step
    parses `rg` output while the build needs `cargo`. On a machine where either lives
    outside `/usr/bin` (rustup installs both under `~/.cargo/bin`), a fixed PATH made
    the step report a broken fork surface — a red about the test's environment that
    reads like a red about the gate.
    """
    dirs = ["/usr/bin", "/bin"]
    for tool in ("cargo", "rg"):
        found = shutil.which(tool)
        if found is not None:
            parent = str(pathlib.Path(found).parent)
            if parent not in dirs:
                dirs.insert(0, parent)
    return ":".join(dirs)


class PrepushGateReachability(unittest.TestCase):
    def test_the_gate_runs_past_its_prologue(self):
        # A tiny disk floor: the assertion is about reaching the steps, and the real
        # floor (15 GB) would make this test depend on the machine's free space.
        result = subprocess.run(
            ["bash", str(GATE), "--fast"],
            capture_output=True,
            text=True,
            env={"PATH": gate_path(), "HOME": "/root",
                 "GATE_MIN_FREE_GB": "0", "GATE_UNIT_MIN_FREE_GB": "0"},
            timeout=600,
        )
        combined = result.stdout + result.stderr
        self.assertNotIn(
            "unbound variable",
            combined,
            "the gate's prologue references an unset variable:\n" + combined[:2000],
        )
        # It reached its steps. A red gate is fine here; an unrunnable one is not.
        self.assertIn(
            "=== fmt",
            combined,
            "the gate never reached its first step:\n" + combined[:2000],
        )

    def test_the_target_liveness_check_is_declared_and_called(self):
        script = GATE.read_text()
        self.assertIn("check_target_liveness()", script)
        self.assertIn('step "target liveness"', script)
        # A guard that is declared and never called cannot fail, which is the class the
        # gate's own comments say testgov exists for.
        self.assertGreaterEqual(
            script.count("check_target_liveness"), 2, "declared and called"
        )

    def test_the_root_assignment_survives(self):
        """The exact regression: `export ROOT` without `ROOT=` is not an assignment."""
        script = GATE.read_text()
        self.assertIn('ROOT="$PWD"', script)
        for line in script.splitlines():
            stripped = line.strip()
            if stripped == "export ROOT":
                self.assertIn(
                    'ROOT="$PWD"',
                    script,
                    "`export ROOT` needs an assignment somewhere",
                )

def summary_source():
    """The gate's own post-run summary, as text, so its verdicts can be run for real."""
    body = GATE.read_text()
    start = body.index("ran_ok=$(grep -c")
    end = body.index('if [[ $fail -ne 0 ]]; then\n  echo; echo "GATE RED. Logs')
    return body[start:end]


def run_unit_source():
    """The gate's own `run_unit`, as text, so its verdicts can be exercised for real."""
    lines = GATE.read_text().splitlines()
    start = lines.index("run_unit() {")
    for index in range(start + 1, len(lines)):
        if lines[index] == "}":
            return "\n".join(lines[start : index + 1]) + "\n"
    raise AssertionError("run_unit is never closed at column 0")


# A canned nextest transcript plus the exit status that goes with it. The stub `cargo`
# below prints the transcript instead of running one: these tests are about the VERDICT
# the gate draws from nextest's output, not about compiling the workspace.
NEXTEST_ALL_IGNORED = """\
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.28s
 Nextest run ID 7cdef8d4 with nextest profile: default
    Starting 0 tests across 1 binary (1 test skipped)
     Summary [   0.005s] 0 tests run: 0 passed, 1 skipped
warning: no tests to run
"""
NEXTEST_NOTHING_LISTED = """\
    Starting 0 tests across 1 binary
     Summary [   0.001s] 0 tests run: 0 passed, 0 skipped
warning: no tests to run
"""
NEXTEST_PASSED = """\
    Starting 12 tests across 1 binary
     Summary [   0.402s] 12 tests run: 12 passed, 0 skipped
"""
NEXTEST_FAILED = """\
    Starting 12 tests across 1 binary
        FAIL [   0.010s] pa-diff::a_test_that_breaks
     Summary [   0.402s] 12 tests run: 11 passed, 1 failed, 0 skipped
"""
CARGO_ALL_IGNORED = """\
running 1 test
test the_only_test ... ignored, networked: opt in
test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s
"""
CARGO_PASSED = """\
running 12 tests
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s
"""
CARGO_NO_TESTS = """\
running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
"""


class RunUnitVerdicts(unittest.TestCase):
    """Review #53: an all-`#[ignore]`d unit exited 0 and printed `ok`.

    nextest's `--no-tests=warn` turns "this unit ran nothing" into a successful exit, so
    the gate certified two units (`pa-jev#test:live_subq`, `pa-core#test:jev_live_eval`)
    that never executed a test. These tests run the real `run_unit` against a stub
    `cargo` that prints a canned transcript, so the verdict is observed, not asserted
    about in the script's text.
    """

    def verdict(self, unit, transcript, status):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / "out" / "tests").mkdir(parents=True)
            (root / "bin").mkdir()
            transcript_file = root / "transcript.txt"
            transcript_file.write_text(transcript)
            cargo = root / "bin" / "cargo"
            cargo.write_text(
                "#!/bin/bash\n"
                f"cat '{transcript_file}'\n"
                f"exit {status}\n"
            )
            cargo.chmod(0o755)
            nextest = root / "bin" / "cargo-nextest"
            nextest.write_text("#!/bin/bash\n")
            nextest.chmod(0o755)
            script = root / "run_unit.sh"
            script.write_text(
                "set -uo pipefail\n" + run_unit_source() + f"run_unit '{unit}'\n"
            )
            result = subprocess.run(
                ["bash", str(script)],
                capture_output=True,
                text=True,
                env={
                    "PATH": f"{root / 'bin'}:/usr/bin:/bin",
                    "ROOT": str(root),
                    "OUT": str(root / "out"),
                    "UNIT_MIN_FREE_GB": "0",
                },
                timeout=120,
            )
            return result.returncode, result.stdout

    def test_an_all_ignored_unit_is_a_skip_not_a_pass(self):
        rc, out = self.verdict("pa-jev#test:live_subq", NEXTEST_ALL_IGNORED, 0)
        self.assertEqual(rc, 0, "a legitimately ignored unit must not turn the gate red")
        self.assertNotIn("ok ", out, f"an untested unit reported as tested:\n{out}")
        self.assertTrue(out.startswith("skip "), out)
        self.assertIn("#[ignore]", out)

    def test_a_target_that_lists_no_tests_is_a_skip_with_its_own_reason(self):
        rc, out = self.verdict("pa-jev#test:live_subq", NEXTEST_NOTHING_LISTED, 0)
        self.assertEqual(rc, 0, out)
        self.assertTrue(out.startswith("skip "), out)
        self.assertIn("no tests", out)
        self.assertNotIn("#[ignore]", out)

    def test_the_older_nextest_belt_still_skips(self):
        """Exit 4 without `--no-tests` is the same zero-test case, and still a skip."""
        rc, out = self.verdict("pa-jev#test:live_subq", NEXTEST_NOTHING_LISTED, 4)
        self.assertEqual(rc, 0, out)
        self.assertTrue(out.startswith("skip "), out)

    def test_a_passing_unit_is_still_ok(self):
        rc, out = self.verdict("pa-diff#lib:pa_diff", NEXTEST_PASSED, 0)
        self.assertEqual(rc, 0, out)
        self.assertTrue(out.startswith("ok "), out)

    def test_a_failing_unit_is_still_fail(self):
        rc, out = self.verdict("pa-diff#lib:pa_diff", NEXTEST_FAILED, 100)
        self.assertEqual(rc, 1, out)
        self.assertTrue(out.startswith("FAIL "), out)

    def test_cargo_tests_all_ignored_unit_is_a_skip_too(self):
        """The doc/bin path runs cargo test, whose `0 passed; 0 failed; 1 ignored`
        is the same "nothing executed" shape."""
        rc, out = self.verdict("pa-jev#doc:doc", CARGO_ALL_IGNORED, 0)
        self.assertEqual(rc, 0, out)
        self.assertTrue(out.startswith("skip "), out)

    def test_cargo_tests_target_with_no_tests_is_a_skip_with_its_own_reason(self):
        rc, out = self.verdict("pa-telemetry#doc:doc", CARGO_NO_TESTS, 0)
        self.assertEqual(rc, 0, out)
        self.assertTrue(out.startswith("skip "), out)
        self.assertIn("no tests", out)

    def test_cargo_tests_passing_unit_is_still_ok(self):
        rc, out = self.verdict("pa-jev#doc:doc", CARGO_PASSED, 0)
        self.assertEqual(rc, 0, out)
        self.assertTrue(out.startswith("ok "), out)

    def summary_for(self, lines, units):
        """Run the gate's real summary block over a canned `summary.log`."""
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / "tests").mkdir()
            (root / "tests" / "summary.log").write_text("\n".join(lines) + "\n")
            script = root / "summary.sh"
            script.write_text(
                "set -uo pipefail\n"
                f"OUT='{root}'\n"
                f"units={units}\n"
                "xargs_status=0\n"
                "test_fail=$(grep -c '^FAIL' \"$OUT/tests/summary.log\" || true)\n"
                "fail=0\n"
                + summary_source()
                + 'echo "fail=$fail"\n'
            )
            result = subprocess.run(
                ["bash", str(script)], capture_output=True, text=True, timeout=60
            )
            return result.returncode, result.stdout

    def test_the_summary_counts_skipped_units_apart_from_ok(self):
        """`all N units green` was the same sentence a healthy run printed, so the
        summary must name the skip count and the units it did not execute."""
        rc, out = self.summary_for(
            ["ok   pa-diff lib pa_diff", "skip pa-jev test live_subq"], 2
        )
        self.assertEqual(rc, 0, out)
        self.assertIn("2 of 2 units accounted for (1 ok, 1 skipped:", out)
        self.assertNotIn("units green", out, f"a skipped unit still reads as green:\n{out}")
        self.assertIn(
            "0 with no tests to run", out,
            f"the summary must not confuse 'ignored' with 'nothing to run':\n{out}",
        )
        self.assertIn("skip pa-jev test live_subq", out, out)
        self.assertIn("fail=0", out, "a skipped unit must not turn the gate red")

    def test_the_summary_names_ignored_units_but_not_empty_targets(self):
        """A doctest target with nothing to document is noise; a unit whose tests were
        skipped is the thing the reader must not mistake for tested."""
        rc, out = self.summary_for(
            [
                "ok   pa-diff lib pa_diff",
                "skip pa-jev test live_subq  (every test is #[ignore]d)",
                "skip pa-types doc doc  (the target lists no tests)",
            ],
            3,
        )
        self.assertEqual(rc, 0, out)
        self.assertIn("(1 ok, 2 skipped: 1 with no tests to run)", out)
        # `tail -3` echoes the summary log above the counts, so the listing is what
        # follows the count line.
        listing = out[out.index("units accounted for") :]
        self.assertIn("skip pa-jev test live_subq", listing, listing)
        self.assertNotIn(
            "skip pa-types doc doc", listing,
            f"an empty target is counted, not listed:\n{listing}",
        )

    def test_a_healthy_summary_still_says_every_unit_ran(self):
        rc, out = self.summary_for(["ok   pa-diff lib pa_diff"], 1)
        self.assertEqual(rc, 0, out)
        self.assertIn("1 of 1 units accounted for (1 ok, 0 skipped:", out)
        self.assertIn("fail=0", out)



if __name__ == "__main__":
    unittest.main()
