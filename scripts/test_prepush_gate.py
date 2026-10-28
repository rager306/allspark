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
import subprocess
import unittest

GATE = pathlib.Path(__file__).resolve().parent / "fork" / "prepush-gate.sh"


class PrepushGateReachability(unittest.TestCase):
    def test_the_gate_runs_past_its_prologue(self):
        # A tiny disk floor: the assertion is about reaching the steps, and the real
        # floor (15 GB) would make this test depend on the machine's free space.
        result = subprocess.run(
            ["bash", str(GATE), "--fast"],
            capture_output=True,
            text=True,
            env={"PATH": "/usr/local/bin:/usr/bin:/bin", "HOME": "/root",
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


if __name__ == "__main__":
    unittest.main()
