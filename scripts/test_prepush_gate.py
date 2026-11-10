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
import os
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



LANE_STEP = 'step "verus proofs and proof depth (advisory)"'
DRIFT_STEP = 'step "verus mirror drift (advisory)"'


def slice_between(body, start_marker, end_marker, what):
    """`body[start:end]`, with a failure that names the missing piece.

    A plain `.index()` raises `ValueError: substring not found`, which reads as a broken
    test rather than as the defect: this gate has no such step. The message says which.
    """
    assert start_marker in body, f"the gate has no {what} step ({start_marker})"
    assert end_marker in body, f"the gate's {what} step has no known end ({end_marker})"
    return body[body.index(start_marker) : body.index(end_marker)]


def verus_lane_source():
    """The gate's advisory Verus lane, as text, so its verdicts can be run for real."""
    return slice_between(
        GATE.read_text(), LANE_STEP, 'step "loom (pa-mem interleavings)"', "proof-lane"
    )


def verus_drift_source():
    """The cheap drift check that runs before the gate's `--fast` exit."""
    return slice_between(
        GATE.read_text(), DRIFT_STEP, 'if [[ $FAST -eq 1 ]]; then', "mirror-drift"
    )


class VerusAdvisoryLane(unittest.TestCase):
    """The proof lane was wired NOWHERE, and its one wired step contradicted its own rule.

    Two defects, both measured on 2026-11-01:
      * `scripts/verus-check.sh` — the script that proves the mirrors and then diffs
        their constants — had no caller outside its own docs. The only Verus step the
        gate ran was the mutation lane, so a broken proof stayed invisible.
      * that one step set `fail=1`, so Verus WAS blocking, while AGENTS.md says a proof
        that does not close never blocks a merge.

    These tests run the gate's real lane against stub scripts, so both the wiring and
    the advisory verdict are observed rather than asserted about in the script's text.
    """

    CLEAN_PROOFS = "verus-check: all mirror specs verified"
    CLEAN_MUTANTS = "verus-mutants: ok (7 killed by Verus); 100.0% kill rate; sample=16 of 34"

    def run_lane(self, statuses, messages):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / "out").mkdir()
            (root / "scripts").mkdir()
            for status, name, message in zip(
                statuses, ("verus-check.sh", "verus-mutants.sh"), messages
            ):
                stub = root / "scripts" / name
                stub.write_text(
                    "#!/bin/bash\nprintf '%s\\n' "
                    + " ".join(f"'{line}'" for line in message.splitlines())
                    + f"\nexit {status}\n"
                )
                stub.chmod(0o755)
            script = root / "lane.sh"
            script.write_text(
                "set -uo pipefail\nfail=0\nstep() { printf '=== %s\\n' \"$*\"; }\n"
                + verus_lane_source()
                + 'echo "fail=$fail"\n'
            )
            result = subprocess.run(
                ["bash", str(script)],
                capture_output=True,
                text=True,
                # The lane invokes its stubs by the RELATIVE paths the gate uses
                # (`bash scripts/verus-check.sh`), so the run's cwd is the stub tree.
                cwd=root,
                env={"PATH": "/usr/bin:/bin", "OUT": str(root / "out")},
                timeout=120,
            )
            return result.returncode, result.stdout

    def test_the_lane_runs_the_proof_check_and_not_only_the_mutation_one(self):
        """The wiring defect: `verus-check.sh` had no caller, so a failing proof was
        reported by nobody while the mutation lane (which passes) printed `ok`."""
        rc, out = self.run_lane(
            (1, 0),
            ("verus-check: FAILED pa_core_gates.rs", self.CLEAN_MUTANTS),
        )
        self.assertEqual(rc, 0, out)
        self.assertIn("verus-check (proofs): FAILED", out, out)
        self.assertIn("verus-mutants (proof depth): ok", out, out)
        self.assertIn("1 advisory failure(s)", out, out)

    def test_a_failed_lane_is_advisory_and_never_turns_the_gate_red(self):
        """AGENTS.md: a proof that does not close must never block a merge. The
        mutation lane used to set `fail`, which is the reverse defect."""
        for statuses in ((1, 1), (0, 1)):
            rc, out = self.run_lane(
                statuses,
                (
                    "verus-check: FAILED pa_core_gates.rs",
                    "verus-mutants: FAILED — the kill rate fell below the floor (a real survivor)",
                ),
            )
            self.assertEqual(rc, 0, out)
            self.assertIn("fail=0", out, f"{statuses} blocked the gate:\n{out}")

    def test_a_missing_toolchain_skips_with_its_reason(self):
        """AGENTS.md: `verus-check.sh` exits 0 when the toolchain is absent. That skip
        must read as a skip, not as a clean pass and not as a failure."""
        rc, out = self.run_lane(
            (0, 0),
            (
                "verus-check: toolchain not found at /x — skipping (install: verus/README.md)",
                "verus-mutants: no Verus toolchain at verus/toolchain — skipping (see verus/README.md)",
            ),
        )
        self.assertEqual(rc, 0, out)
        self.assertIn("verus-check (proofs): SKIPPED", out, out)
        self.assertIn("verus-mutants (proof depth): SKIPPED", out, out)
        self.assertIn("verus: advisory lanes clean", out, out)

    def test_an_infrastructure_failure_is_not_reported_as_a_survivor(self):
        """Measured 2026-11-01: the gate exports `TMPDIR="$ROOT/.tmp"`, and
        `cargo-verus-mutants` copies its package under `$TMPDIR/.tmpXXXX/source`,
        which is then INSIDE the workspace — `cargo metadata` refuses it and the
        baseline build dies. `verus-mutants.sh` reports that as "the kill rate fell
        below the floor (a real survivor)", a claim about the proofs that the
        evidence does not support: they were never run."""
        rc, out = self.run_lane(
            (0, 1),
            (
                self.CLEAN_PROOFS,
                "verus-mutants: FAILED — the kill rate fell below the floor (a real survivor)\n"
                "Error: baseline failed: cargo verus build -p allspark-verus-mirrors\n"
                "Error: `cargo metadata` exited with an error: error: current package"
                " believes it's in a workspace when it's not:",
            ),
        )
        self.assertEqual(rc, 0, out)
        self.assertIn("INFRASTRUCTURE FAILURE", out, out)
        self.assertNotIn("a real survivor", out, "the lane's own wrong verdict was repeated")
        self.assertIn("verus: advisory lanes clean", out, "not counted as a proof failure")

    def test_the_mutation_lane_reports_its_own_verdict(self):
        """`verus-mutants.sh` shares its log path with the step's redirect, so the file
        ends on cargo's `summary: <path>` line. Reporting `tail -1` put a file name
        where the kill rate belongs."""
        rc, out = self.run_lane(
            (0, 0),
            (
                self.CLEAN_PROOFS,
                self.CLEAN_MUTANTS
                + "\n16 mutants\nsummary: /repo/verus/mirrors/target/verus-mutants/summary.json",
            ),
        )
        self.assertEqual(rc, 0, out)
        self.assertIn("100.0% kill rate", out, out)
        self.assertNotIn("verus-mutants (proof depth): ok — summary:", out, out)


class TheGateRunsItsOwnTests(unittest.TestCase):
    """This file guarded the gate and nothing ran it.

    The gate's classes of defect are exactly "a guard that exists and is never
    invoked" (`UntrackedOracle`, the disk floor, the catalog waiver, the Verus lane).
    This file was the same shape one level up: `scripts/test_prepush_gate.py` was
    written, committed, and referenced by no Makefile target, workflow or script, so
    every verdict below could rot without a signal.
    """

    def test_the_gate_runs_this_file(self):
        script = GATE.read_text()
        self.assertIn("scripts/test_prepush_gate.py", script)
        for class_name in (
            "RunUnitVerdicts",
            "VerusAdvisoryLane",
            "VerusMirrorDriftReachability",
            "FastExitRespectsTheContractVerdict",
            "ParitySkipsAreCounted",
        ):
            self.assertIn(class_name, script, f"the gate skips {class_name}")

    def test_the_fast_lanes_run_before_the_fast_exit(self):
        """`--fast` is the run developers make; a contract check after its exit is the
        same dead wiring this file exists to catch."""
        script = GATE.read_text()
        self.assertIn(
            "scripts/test_prepush_gate.py",
            script,
            "the gate does not run its own tests at all",
        )
        self.assertLess(
            script.index("scripts/test_prepush_gate.py"),
            script.index('if [[ $FAST -eq 1 ]]; then'),
            "the gate's own contract check must run in --fast too",
        )


class ParitySkipsAreCounted(unittest.TestCase):
    """Finding #1: a parity test with no TS reference exits 0, so the gate read it as a pass.

    Two defects, each measured, each pinned here:

    1. The unit is reported `ok` although nothing ran — a test that skips returns normally
       and prints its own SKIPPED line (cargo has no runtime skip).
    2. The FIRST fix grepped that line out of the unit logs and counted zero forever:
       `eprintln!` from a PASSING test is captured by nextest and by cargo test and never
       written to the log (0 `SKIPPED` lines by default, 3 with `--success-output=final`).
       So the revealing flag has to be passed for the units that can skip.

    The count stays advisory (the skip contract is deliberate); what must not happen is the
    absence reading as a green.
    """

    COUNT_STEP = "# A test that skips at RUNTIME returns NORMALLY"
    END = 'if [[ $fail -ne 0 ]]; then'

    def run_block(self, logs):
        script = GATE.read_text()
        self.assertIn(self.COUNT_STEP, script, "the gate has no parity-skip counter")
        start = script.index(self.COUNT_STEP)
        # The end marker is searched AFTER the start: `if [[ $fail -ne 0 ]]` appears
        # earlier in the file (after fmt/clippy), and a plain `index` would slice backwards
        # into an empty string — a test that then asserts nothing about anything.
        block = script[start : script.index(self.END, start)]
        with tempfile.TemporaryDirectory() as tmp:
            out = pathlib.Path(tmp)
            (out / "tests").mkdir()
            for name, body in logs.items():
                (out / "tests" / name).write_text(body)
            result = subprocess.run(
                ["bash", "-c", f"OUT={out}; " + block],
                capture_output=True,
                text=True,
                timeout=60,
            )
            return result.returncode, result.stdout + result.stderr

    def run_unit_argv(self, unit):
        """The exact argv `run_unit` hands the runner, captured by a stub `cargo`."""
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / "out" / "tests").mkdir(parents=True)
            (root / "bin").mkdir()
            argv_file = root / "argv.txt"
            cargo = root / "bin" / "cargo"
            cargo.write_text(
                "#!/bin/bash\n"
                f"printf '%s\\n' \"$@\" > '{argv_file}'\n"
                "echo 'test result: ok. 3 passed; 0 failed; 0 ignored'\n"
                "exit 0\n"
            )
            cargo.chmod(0o755)
            # Both runners present: the nextest branch is the one the gate takes for a
            # `test` unit, and it is the branch with the reveal flag.
            nextest = root / "bin" / "cargo-nextest"
            nextest.write_text("#!/bin/bash\nexit 0\n")
            nextest.chmod(0o755)
            script = root / "run_unit.sh"
            script.write_text("set -uo pipefail\n" + run_unit_source() + f"run_unit '{unit}'\n")
            subprocess.run(
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
            return argv_file.read_text().splitlines()

    def test_the_skipped_comparisons_are_named(self):
        code, out = self.run_block(
            {
                "pa-cli-test-lock_compat_e2e.log": "SKIPPED: TS prime-agent binary not found\n",
                "pa-cli-lib-pa_cli.log": "test result: ok. 12 passed;\n",
            }
        )
        self.assertEqual(code, 0, out)
        self.assertIn("1 test(s) skipped at runtime", out)

    def test_two_units_skipping_are_summed(self):
        code, out = self.run_block(
            {
                "pa-cli-test-lock_compat_e2e.log": "SKIPPED: TS prime-agent binary not found\n",
                "pa-jev-test-live_subq.log": "SKIPPED: live SubQ smoke\n",
            }
        )
        self.assertEqual(code, 0, out)
        self.assertIn("2 test(s) skipped at runtime", out)

    def test_the_count_survives_a_single_log(self):
        """`grep -c` over ONE file prints a bare count with no colon; `$2` summed zero."""
        code, out = self.run_block(
            {"pa-cli-test-lock_compat_e2e.log": "SKIPPED: TS prime-agent binary not found\n"}
        )
        self.assertEqual(code, 0, out)
        self.assertIn("1 test(s) skipped at runtime", out)

    def test_a_run_with_no_skips_says_nothing(self):
        code, out = self.run_block({"pa-cli-lib-pa_cli.log": "test result: ok. 12 passed;\n"})
        self.assertEqual(code, 0, out)
        self.assertNotIn("skipped at runtime", out)

    def test_the_parity_units_reveal_a_passing_tests_output(self):
        """Without the flag the SKIPPED line is captured and the counter counts zero."""
        for unit in (
            "pa-cli#test:lock_compat_e2e",
            "pa-jev#test:live_subq",
            "pa-core#test:jev_live_eval",
        ):
            argv = self.run_unit_argv(unit)
            self.assertTrue(
                any(arg.startswith("--success-output") for arg in argv),
                f"{unit} does not reveal a passing test's output: {argv}",
            )

    def test_an_ordinary_unit_pays_nothing_for_it(self):
        """Measured: the flag takes a unit's log x4 and its wall clock +12%."""
        argv = self.run_unit_argv("pa-graph#lib:pa_graph")
        self.assertFalse(
            any(arg.startswith("--success-output") for arg in argv),
            f"an ordinary unit was made to pay for the parity counter: {argv}",
        )


class FastExitRespectsTheContractVerdict(unittest.TestCase):
    """`--fast` printed GATE GREEN and returned 0 after its own contract tests failed.

    The contract lane above it sets `fail=1`, and the fast exit never looked: fmt and
    clippy were checked before that lane, so the only accumulated verdict at the exit
    came from the gate's own tests — and it was ignored. A developer who ran the fast
    gate was told the gate was green by a gate whose guard had just gone red, which is
    the class this file exists to catch, one level up.

    The test runs the exit's REAL text twice, with `fail` set each way, so the verdict
    is observed rather than asserted about in a comment.
    """

    START = "# The accumulated verdict, not the formality of reaching this line"
    END = "# Verification-ladder layers below clippy"

    def run_exit(self, fail_value):
        script = GATE.read_text()
        self.assertIn(self.START, script, "the fast exit has no verdict block")
        block = script[script.index(self.START) : script.index(self.END)]
        with tempfile.TemporaryDirectory() as tmp:
            runner = pathlib.Path(tmp) / "exit.sh"
            runner.write_text(
                f"set -uo pipefail\nfail={fail_value}\nFAST=1\nOUT=/tmp/x\n"
                + block
                + 'echo "REACHED-THE-END\"\n'
            )
            result = subprocess.run(
                ["bash", str(runner)], capture_output=True, text=True, timeout=60
            )
            return result.returncode, result.stdout + result.stderr

    def test_a_failed_contract_makes_the_fast_gate_red(self):
        code, out = self.run_exit(1)
        self.assertEqual(code, 1, f"a red contract must fail the fast gate: {out}")
        self.assertIn("GATE RED", out)

    def test_a_green_contract_still_lets_the_fast_gate_pass(self):
        code, out = self.run_exit(0)
        self.assertEqual(code, 0, f"a green contract must not be turned red: {out}")
        self.assertIn("GATE GREEN (fast", out)


class VerusMirrorDriftReachability(unittest.TestCase):
    """The drift check must reach the run developers actually do.

    `--fast` exits right after fmt and clippy. A drift check placed among the layered
    verification steps below that exit is unreachable from `--fast`, which is the gate
    most runs use — the same shape as the defect being fixed: a check that exists and
    is never reached.
    """

    def run_drift(self, status, message):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / "out").mkdir()
            (root / "scripts").mkdir()
            stub = root / "scripts" / "verus-mirror-check.py"
            stub.write_text(
                "#!/usr/bin/env python3\nprint("
                + " + chr(10) + ".join(repr(line) for line in message.splitlines())
                + f")\nraise SystemExit({status})\n"
            )
            stub.chmod(0o755)
            script = root / "drift.sh"
            script.write_text(
                "set -uo pipefail\nfail=0\nstep() { printf '=== %s\\n' \"$*\"; }\n"
                + verus_drift_source()
                + 'echo "fail=$fail"\n'
            )
            result = subprocess.run(
                ["bash", str(script)],
                capture_output=True,
                text=True,
                cwd=root,
                env={"PATH": "/usr/bin:/bin", "OUT": str(root / "out")},
                timeout=120,
            )
            return result.returncode, result.stdout

    def test_the_drift_check_precedes_the_fast_exit(self):
        """The reachability half: a drift check after the `--fast` exit is unreachable
        from the run most developers make."""
        script = GATE.read_text()
        assert DRIFT_STEP in script, "the gate has no mirror-drift step at all"
        self.assertLess(
            script.index(DRIFT_STEP),
            script.index('if [[ $FAST -eq 1 ]]; then'),
            "the only toolchain-free Verus check must run in --fast too",
        )

    def test_a_drift_is_reported_with_the_constants_named(self):
        rc, out = self.run_drift(
            1,
            "verus-mirror-check: DRIFT PTR_BYTES: verus/pa_mem_bucket.rs says 6,"
            " crates/pa-mem/src/ptr48.rs says 8",
        )
        self.assertEqual(rc, 0, out)
        self.assertIn("verus-mirror: DRIFT", out, out)
        self.assertIn("DRIFT PTR_BYTES", out, "the drift itself must reach the reader")
        self.assertIn("fail=0", out, "a drift is advisory by AGENTS.md")

    def test_agreement_is_reported_as_agreement(self):
        rc, out = self.run_drift(0, "verus-mirror-check: mirrors agree with their sources")
        self.assertEqual(rc, 0, out)
        self.assertIn("verus-mirror: ok", out, out)


class VerusMutantsVerdict(unittest.TestCase):
    """`verus-mutants.sh` must not report a kill rate when no proof was attempted.

    The lane runs `cargo verus-mutants`, which copies the package into
    `$TMPDIR/.tmpXXXX/source`. Under the gate `TMPDIR` is the repo's own `.tmp`, so
    that copy lands inside the workspace and `cargo metadata` refuses it with
    "current package believes it's in a workspace when it's not" — the tool then finds
    no Verus package at all. The script used to answer every non-timeout failure with
    "the kill rate fell below the floor (a real survivor)", which asserts something
    about proof depth that the run never measured.

    Found 2026-11-01: with the gate's TMPDIR the lane exited 1 claiming a survivor,
    while the same tree with `env -u TMPDIR` reported "7 killed, 100.0%".
    """

    SCRIPT = pathlib.Path(__file__).resolve().parent / "verus-mutants.sh"

    def verdict_for(self, tool_output):
        """Run the script's verdict arms against a stub `cargo`, verbatim from the file."""
        body = self.SCRIPT.read_text()
        # The arms after the invocation: everything from the first `if PATH=` line on.
        start = body.index('if PATH="$TOOLCHAIN:$PATH" cargo verus-mutants')
        arms = body[start:]

        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / "bin").mkdir()
            # A stub `cargo` that writes the tool's output to $LOG and exits 1.
            cargo = root / "bin" / "cargo"
            cargo.write_text(
                "#!/bin/sh\n"
                f"cat >&2 <<'EOF'\n{tool_output}\nEOF\n"
                "exit 1\n"
            )
            cargo.chmod(0o755)
            script = root / "run.sh"
            script.write_text(
                'TOOLCHAIN=""\n'
                f'LOG="{root}/log"\n'
                'LIMIT=16\n'
                + arms
            )
            env = dict(os.environ)
            env["PATH"] = f"{root / 'bin'}:{env.get('PATH', '')}"
            done = subprocess.run(
                ["bash", str(script)],
                capture_output=True,
                text=True,
                env=env,
                cwd=root,
            )
            return done.returncode, done.stdout + done.stderr

    def test_a_baseline_that_never_built_is_infrastructure_not_a_survivor(self):
        rc, out = self.verdict_for(
            "Error: `cargo metadata` exited with an error: error: current package "
            "believes it's in a workspace when it's not"
        )
        self.assertIn("SKIPPED", out, out)
        self.assertNotIn("real survivor", out, "nothing was proved, so nothing survived")

    def test_no_verus_package_is_also_infrastructure(self):
        rc, out = self.verdict_for(
            "Error: found no Verus workspace packages; expected "
            "package.metadata.verus.verify = true"
        )
        self.assertIn("SKIPPED", out, out)
        self.assertNotIn("kill rate fell below", out, out)

    def test_a_real_survivor_still_reads_as_a_survivor(self):
        rc, out = self.verdict_for("mutation survived: 3 of 16\n40.0% kill rate")
        self.assertNotEqual(rc, 0, out)
        self.assertIn("real survivor", out, out)


if __name__ == "__main__":
    unittest.main()
