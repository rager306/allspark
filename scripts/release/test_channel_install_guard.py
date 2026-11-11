#!/usr/bin/env python3
"""The release installer harnesses refuse to run from inside a live session.

`scripts/release/test_channel_install.py` and `test_legacy_upgrade.py` spawn the
REAL `install-rust.sh`, which stops daemons BY SOCKET PATH. AGENTS.md forbids
running that installer from inside a prime-agent session "in any form", because
the installer's stop probe is exactly the mechanism that reaches this session's
supervisor — and the harnesses deliberately scrub the environment down to an
allowlist, which is the isolation posture the rule says makes a destructive probe
MORE likely, not safer.

So the harness itself fails closed, before an environment is built:

  * the guard reads the SESSION markers from the PARENT process (before the
    allowlist drops them) and refuses — and, because those markers are exactly
    what a scrubbed environment (`env -i`) removes, it ALSO walks the process tree
    for the daemon the run lives under (`daemon_ancestor_socket`, the same
    mechanism `install-rust.sh`'s own probe uses);
  * on a non-Linux host an over-long fixture TMPDIR makes the installer fall
    back to `/tmp/<name>-<uid>` — the ambient namespace — so the guard refuses a
    fixture socket that long instead of relying on TMPDIR to contain it;
  * the guard also refuses a socket path that resolves outside the fixture root.

And the probe resolves its mode instead of assuming one: the `auto` default
answers `fixture` inside a session — publishing a stand-in installer that installs
a fake launcher and opens no socket — and `real` on a release runner, so the
CHANNEL flow (publish, download, install, reinstall, user-state preservation) stays
under test inside a session while `release.yml`, which passes no mode flag, still
exercises the shipped installer end to end. A test holds that mapping.

Run: python3 scripts/release/test_channel_install_guard.py  (make install-harness-gates)
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import test_legacy_upgrade as harness  # noqa: E402


class TheSessionGuardTests(unittest.TestCase):

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "tmp").mkdir()

    def session_env(self) -> dict:
        """ONE live session marker (measured on 2026-11-09), the others cleared.

        Both are named, because `patch.dict` only rewrites the keys it is given:
        this battery may run under a real daemon (it does, under an agent), and an
        ambient worker marker would keep a case green whichever arm was mutated.
        """
        return {"PRIME_AGENT_INTERNAL_DAEMON_SUPERVISOR_SOCKET":
                "/tmp/prime-agent-rust-0/daemon.sock",
                "PRIME_AGENT_INTERNAL_DAEMON_WORKER": ""}

    def outside_a_session(self):
        # These cases are about the OTHER arms, and this battery may itself run
        # inside a session (it does, under an agent): clear the markers so each
        # case exercises one rule, not the session refusal every time.
        return patch.dict(os.environ,
                          {"PRIME_AGENT_INTERNAL_DAEMON_WORKER": "",
                           "PRIME_AGENT_INTERNAL_DAEMON_SUPERVISOR_SOCKET": ""})

    def markers_only(self):
        """The marker arm ON ITS OWN: no daemon ancestor to refuse for it.

        This battery runs under a live daemon (it is an agent session), so without
        this patch the process-tree arm answers for every case here and a marker
        regression stays green — the discrimination the tests exist for.
        """
        return patch.object(harness, "daemon_ancestor_socket", return_value=None)

    def test_a_session_invocation_is_refused_before_an_environment_is_built(self):
        with patch.dict(os.environ, self.session_env()), self.markers_only():
            with self.assertRaises(SystemExit) as caught:
                harness.isolated_environment(self.root / "user", "http://127.0.0.1:1",
                                             live_installer=True)
        self.assertIn("inside a prime-agent session", str(caught.exception))

    def test_the_worker_marker_alone_is_enough(self):
        # The supervisor socket variable is one marker; the worker role is the
        # other, and a worker's parent has both — but neither is guaranteed.
        with patch.dict(os.environ,
                        {"PRIME_AGENT_INTERNAL_DAEMON_WORKER": "1",
                         "PRIME_AGENT_INTERNAL_DAEMON_SUPERVISOR_SOCKET": "",
                         "PRIME_AGENT_DAEMON_SOCKET": ""}), \
                self.markers_only():
            with self.assertRaises(SystemExit):
                harness.isolated_environment(self.root / "user", "http://127.0.0.1:1",
                                             live_installer=True)

    def test_a_long_fixture_path_is_refused_on_a_non_linux_host(self):
        # Above the unix sun_path limit the installer falls back to /tmp — the
        # ambient namespace, where the operator's own daemon lives.
        deep = self.root / ("d" * 60) / ("e" * 60)
        deep.mkdir(parents=True)
        with self.outside_a_session(), self.assertRaises(SystemExit) as caught:
            harness.guard_isolated_sockets(deep, platform="Darwin")
        self.assertIn("AMBIENT", str(caught.exception))

    def test_the_same_long_path_is_fine_on_linux(self):
        # Linux re-anchors deep unix paths through /proc, so no /tmp fallback is
        # reachable there: refusing would be a false positive in every CI run.
        deep = self.root / ("d" * 60) / ("e" * 60)
        deep.mkdir(parents=True)
        with self.outside_a_session():
            harness.guard_isolated_sockets(deep, platform="Linux")

    def test_the_harnesses_use_a_short_fixture_root(self):
        """A fixture that starts in the host's default temp dir is already too long
        on a macOS runner (`/var/folders/.../T/prime-legacy-upgrade-XXXXXXXX` is 116
        bytes to the socket), and above the limit the installer re-anchors its
        sockets at `/tmp` — the machine's namespace, not the fixture's. So the
        harnesses must create their root through `fixture_root`, which prefers the
        SHORTEST temporary directory the host offers and proves the result fits.
        """
        for harness in ("test_channel_install.py", "test_legacy_upgrade.py"):
            source = (HERE / harness).read_text()
            self.assertNotIn(
                'TemporaryDirectory(prefix="prime-', source,
                f"{harness} must not create its fixture in the default temp dir")
            self.assertIn("fixture_root(", source,
                          f"{harness} must create its fixture through fixture_root")

    def test_a_normal_fixture_root_passes(self):
        with self.outside_a_session():
            harness.guard_isolated_sockets(self.root, platform="Darwin")
            harness.guard_isolated_sockets(self.root, platform="Linux")

    def test_a_scrubbed_environment_does_not_bypass_the_refusal(self):
        """`env -i` drops both markers; the process tree still answers.

        The markers are exported by the daemon, so a scrubbed invocation carries
        neither — which is how the 2026-10-21 incident reached a live supervisor.
        The ancestor walk is the arm that cannot be scrubbed, and
        `isolated_environment` must consult it: with the markers cleared and the
        walk naming the supervisor, the harness still refuses.
        """
        with self.outside_a_session(), \
                patch.object(harness, "daemon_ancestor_socket",
                             return_value="/fixture/supervisor.sock"):
            with self.assertRaises(SystemExit) as caught:
                harness.isolated_environment(self.root / "user-b", "http://127.0.0.1:1",
                                             live_installer=True)
        self.assertIn("under a live daemon", str(caught.exception))

    def test_the_ancestor_walk_names_the_supervisor_and_stops_at_a_plain_tree(self):
        """The walk reads argv/parents through injectable readers, so the rule is
        pinned without depending on what this host happens to run under."""
        tree = {
            100: ["/usr/bin/python3", "-m", "harness"],
            200: ["/root/.local/share/prime-agent/prime-agent", "worker"],
            300: ["/root/.local/share/prime-agent/prime-agent", "--mode", "daemon",
                  "--daemon-socket", "/tmp/prime-agent-rust-0/daemon.sock"],
        }
        parents = {100: 200, 200: 300, 300: 1}
        self.assertEqual(
            harness.daemon_ancestor_socket(
                platform="Linux", ppid=100,
                argv_of=lambda pid: tree[pid], parent_of=lambda pid: parents[pid]),
            "/tmp/prime-agent-rust-0/daemon.sock",
            "the walk passes the worker and the client to reach the supervisor")

        plain = {100: ["/bin/bash"], 200: ["/usr/bin/python3", "-m", "pytest"]}
        plain_parents = {100: 200, 200: 1}
        self.assertIsNone(
            harness.daemon_ancestor_socket(
                platform="Linux", ppid=100,
                argv_of=lambda pid: plain[pid], parent_of=lambda pid: plain_parents[pid]),
            "a plain shell tree names no daemon, so nothing is refused")

        # An unrelated `--mode daemon` shell is not the product: the executable
        # gate is what keeps a wrong refusal out.
        shell = {100: ["sh", "--mode", "daemon", "--daemon-socket", "/tmp/x.sock"]}
        self.assertIsNone(
            harness.daemon_ancestor_socket(
                platform="Linux", ppid=100,
                argv_of=lambda pid: shell[pid], parent_of=lambda pid: 1))

        # The supervisor's other spelling, and an interpreted launcher script
        # (the kernel's shebang exec puts the product name at argv[1]).
        for argv in (
            ["/opt/prime-agent/bin/prime-agent", "supervisor", "--socket", "/tmp/s.sock"],
            ["/bin/sh", "/opt/prime-agent/bin/prime-agent", "--mode", "daemon",
             "--daemon-socket", "/tmp/s.sock"],
        ):
            self.assertEqual(
                harness.daemon_ancestor_socket(
                    platform="Linux", ppid=100,
                    argv_of=lambda _pid, argv=argv: argv, parent_of=lambda _pid: 1),
                "/tmp/s.sock")

    def test_every_installer_invocation_goes_through_the_guard(self):
        """The guard is only worth having where the installer actually starts.

        Both harnesses build their child environment with `isolated_environment`,
        so the guard there covers every `execute(...)` call that runs the shipped
        installer. Pinned as a source contract because the call graph is the
        claim: a new harness that shells the installer with its own env would
        bypass this test's subject.
        """
        with patch.dict(os.environ, self.session_env()):
            with self.assertRaises(SystemExit):
                harness.isolated_environment(self.root / "user-a", "http://127.0.0.1:1",
                                             live_installer=True)
        source = (HERE / "test_channel_install.py").read_text()
        self.assertIn("isolated_environment(root / \"user\", base", source,
                      "the channel harness must build its child env through the guard")
        # And the mode it asks for must be the one it runs: a `fixture` run that
        # still asked for the live-installer gate would refuse inside a session it
        # deliberately made safe, and a `real` run that asked for the fixture gate
        # would start the shipped installer with no refusal at all.
        self.assertIn(
            "live_installer=mode == REAL_INSTALLER", source,
            "the channel harness must pass the live-installer decision from its RESOLVED mode")


class TheReleaseGateKeepsTheRealInstallerTests(unittest.TestCase):
    """The fixture mode must not become the release gate's subject by accident.

    `release.yml` runs OUTSIDE every session (a release runner), and the shipped
    installer's own end-to-end behaviour is the thing that gate exists to prove. So
    the release workflow must invoke the channel probe with no mode flag — the
    `auto` default answers `real` there, and `TheInstallerModeTests` pins that
    mapping — and never with `--installer-mode fixture`, which would turn a real
    install into a fake-launcher install and still report a green gate.
    """

    def test_release_workflows_run_the_channel_probe_without_the_fixture_mode(self):
        repo = HERE.parents[1]
        release = (repo / ".github/workflows/release.yml").read_text()
        self.assertIn("test_channel_install.py", release,
                      "release.yml must run the channel install probe")
        # Every workflow that runs the probe leaves the mode to the `auto` default:
        # asking for the fixture there would turn a real install into a
        # fake-launcher install and still report a green gate.
        ran = 0
        for path in sorted((repo / ".github/workflows").glob("*.yml")):
            text = path.read_text()
            if "test_channel_install.py" not in text:
                continue
            ran += 1
            self.assertNotIn(
                "--installer-mode", text,
                f"{path.name} must run the REAL installer: the fixture mode is for "
                "probes inside a session, not the release gate")
        self.assertGreater(ran, 0, "no workflow runs the channel install probe")

    def test_the_fixture_installer_never_touches_a_daemon_socket(self):
        """The stand-in is only a safe substitute while it stays socket-free."""
        script = harness.FIXTURE_INSTALLER_SCRIPT
        for forbidden in ("install-rust.sh", "daemon.sock", "socket", "kill", "shutdown"):
            self.assertNotIn(forbidden, script,
                             f"the fixture installer must not contain {forbidden!r}")
        self.assertIn("curl -fsSL", script,
                      "the fixture must still exercise the channel download")


class TheInstallerModeTests(unittest.TestCase):
    """`auto` — the DEFAULT — is what a caller who did not think about modes gets.

    The mapped question is a pure function of "does this run live inside a
    session?", so it is pinned here without a daemon ancestor and without a
    socket: the same input the guard reads, one answer, two callers.
    """

    def test_auto_inside_a_session_answers_fixture(self):
        self.assertEqual(
            harness.resolve_installer_mode(harness.AUTO_INSTALLER,
                                           session_reason="under a live daemon (/x.sock)"),
            harness.FIXTURE_INSTALLER)

    def test_auto_outside_a_session_answers_real(self):
        # A release runner: no marker, no daemon ancestor. The gate's subject must
        # stay the SHIPPED installer even though the flag is not spelled out.
        self.assertEqual(
            harness.resolve_installer_mode(harness.AUTO_INSTALLER, session_reason=None),
            harness.REAL_INSTALLER)

    def test_an_explicit_mode_is_obeyed_in_both_directions(self):
        # The release gate names `real`; a test names `fixture`. Neither may be
        # second-guessed by where the run happens to live.
        for session_reason in (None, "inside a prime-agent session (the worker marker)"):
            self.assertEqual(
                harness.resolve_installer_mode(harness.REAL_INSTALLER,
                                               session_reason=session_reason),
                harness.REAL_INSTALLER)
            self.assertEqual(
                harness.resolve_installer_mode(harness.FIXTURE_INSTALLER,
                                               session_reason=session_reason),
                harness.FIXTURE_INSTALLER)

    def test_the_cli_default_is_auto(self):
        """`release.yml` passes no mode flag, so the parser's default decides.

        Read from the shipped parser rather than its source text: the default is a
        behaviour, and a source assertion would keep passing while the value moved.
        """
        import test_channel_install as probe
        args = probe.build_parser().parse_args(
            ["--archive", "a.tar.gz", "--installer", "i.sh", "--report", "r.json"])
        self.assertEqual(args.installer_mode, harness.AUTO_INSTALLER)

    def test_an_unknown_mode_is_refused(self):
        with self.assertRaises(ValueError):
            harness.resolve_installer_mode("yolo", session_reason=None)

class TheFixtureChannelProbeTests(unittest.TestCase):
    """The channel probe's `fixture` mode: the flow without the shipped installer.

    `test_channel_install.py` used to have exactly one way to run: download the
    shipped `install-rust.sh` and execute it. That is the file AGENTS.md forbids
    starting from inside a live session, and the stop-probe reach is the reason.
    The fixture mode is the arm that keeps the CHANNEL under test — publish,
    download, install, reinstall, user-state preservation — while executing a
    stand-in that opens no socket.
    """

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.archive = self.root / "prime-agent-1.0.0-linux-x64.tar.gz"
        import tarfile
        import io
        with tarfile.open(self.archive, "w:gz") as payload:
            body = b"#!/bin/sh\nprintf '1.0.0\\n'\n"
            info = tarfile.TarInfo("prime-agent")
            info.size = len(body)
            info.mode = 0o755
            payload.addfile(info, io.BytesIO(body))
        self.report = self.root / "report.json"
        self.record = self.root / "installer-record"
        # A DECOY installer, never the shipped one. This battery drives the probe's
        # refusal path, and a test that proves a refusal must not become the thing
        # that executes `install-rust.sh` the day the refusal regresses: if the guard
        # is broken, the decoy runs (and writes a marker this file asserts absent)
        # instead of the installer that stops daemons by socket path.
        self.decoy = self.root / "decoy-installer.sh"
        self.decoy.write_text(
            f"#!/bin/sh\nprintf 'decoy\\n' > {self.root / 'DECOY-RAN'}\n")
        self.decoy.chmod(0o755)

    def invoke(self, mode: str | None, *, under_fake_supervisor: bool = False):
        """Run the channel probe, optionally under a daemon-shaped ancestor.

        The ancestor is a shell script NAMED `prime-agent`, spawned with the
        supervisor's argv and running the probe as a child (never `exec`, which
        would replace its argv). That is the shape the ancestor walk reads, so the
        refusal is pinned here without depending on what this host happens to run
        under — and without ever letting the real installer start anywhere, CI
        included. A `None` mode passes no `--installer-mode` at all, which is how
        the release workflow invokes the probe.
        """
        env = {**os.environ, "FIXTURE_INSTALLER_RECORD": str(self.record)}
        env.pop("PRIME_AGENT_INTERNAL_DAEMON_WORKER", None)
        env.pop("PRIME_AGENT_INTERNAL_DAEMON_SUPERVISOR_SOCKET", None)
        probe = [sys.executable, str(HERE / "test_channel_install.py"),
                 "--archive", str(self.archive),
                 "--installer", str(self.decoy),
                 "--report", str(self.report)]
        if mode is not None:
            probe += ["--installer-mode", mode]
        if not under_fake_supervisor:
            return subprocess.run(probe, capture_output=True, text=True, env=env, timeout=300)
        supervisor = self.root / "prime-agent"
        # The stand-in stays the probe's PARENT (no `exec`), so the walk sees its
        # argv; the four leading arguments are its own daemon flags.
        supervisor.write_text('#!/bin/sh\nshift 4\n"$@"\n')
        supervisor.chmod(0o755)
        socket = self.root / "fixture-daemon.sock"
        return subprocess.run(
            [str(supervisor), "--mode", "daemon", "--daemon-socket", str(socket),
             *probe],
            capture_output=True, text=True, env=env, timeout=300)

    def run_probe(self, mode: str | None, *, under_fake_supervisor: bool = False) -> tuple[int, dict]:
        process = self.invoke(mode, under_fake_supervisor=under_fake_supervisor)
        report = json.loads(self.report.read_text()) if self.report.is_file() else {}
        return process.returncode, report

    def test_the_fixture_mode_runs_the_channel_flow_without_the_shipped_installer(self):
        # Under a daemon-shaped ancestor AND with the session markers scrubbed: the
        # fixture mode must not care, because it never starts a socket-probing
        # installer. That is the whole point of the mode.
        status, report = self.run_probe("fixture", under_fake_supervisor=True)
        self.assertEqual(status, 0, report.get("error"))
        self.assertEqual(report.get("status"), "passed", report.get("error"))
        self.assertIs(report.get("fixture_installer_ran"), True,
                      f"the stand-in must be the bytes that ran: {report}")
        self.assertIs(report.get("fresh_install"), True)
        self.assertIs(report.get("reinstall"), True)
        self.assertIs(report.get("user_state_preserved"), True)
        # The isolation the fixture proves: the child never saw a daemon address.
        recorded = (Path(report["installer_record"]) / "env.txt").read_text()
        for leaked in ("PRIME_AGENT_INTERNAL_DAEMON_SUPERVISOR_SOCKET",
                       "PRIME_AGENT_INTERNAL_DAEMON_WORKER",
                       "PRIME_AGENT_DAEMON_SOCKET"):
            self.assertNotIn(leaked, recorded,
                             f"the fixture install inherited {leaked}")

    def test_the_real_mode_refuses_on_the_process_tree_alone(self):
        """The mode that DOES start the shipped installer keeps the refusal — with
        no environment at all.

        The session markers are scrubbed, so the only thing left is the process
        tree: the walk names the daemon-shaped ancestor, and the probe refuses
        before any child runs. Asserted on the report file's absence — a probe that
        got as far as feeding a channel would have written one — and on the
        installer record being untouched.
        """
        if self.report.exists():
            self.report.unlink()
        status, report = self.run_probe("real", under_fake_supervisor=True)
        self.assertNotEqual(status, 0, "a live-session run must fail closed")
        self.assertEqual(report, {}, "no report: the guard refused before the flow")
        self.assertFalse((self.root / "DECOY-RAN").exists(),
                         "no installer ran: the refusal precedes the flow")

    def test_the_default_mode_inside_a_session_runs_the_stand_in(self):
        """NO `--installer-mode` argument, under a daemon-shaped ancestor.

        The default is `auto`, so the probe must answer `fixture` here and run the
        CHANNEL flow without a socket-probing installer — the safe path for a caller
        who did not think about modes. The earlier battery only ever passed a mode
        explicitly, so a default that drifted back to `real` would have kept every
        test green while an in-session run started the shipped script.
        """
        if self.report.exists():
            self.report.unlink()
        status, report = self.run_probe(None, under_fake_supervisor=True)
        self.assertEqual(status, 0, report.get("error"))
        self.assertEqual(report.get("installer_mode"), "fixture", report)
        self.assertEqual(report.get("requested_installer_mode"), "auto", report)
        # The FAKE ancestor named the socket, not this host's own supervisor: the
        # decision must come from the process tree the probe was given.
        reason = report.get("installer_mode_reason", "")
        self.assertIn("under a live daemon", reason)
        self.assertIn("fixture-daemon.sock", reason)
        self.assertIs(report.get("fixture_installer_ran"), True, report)
        self.assertFalse((self.root / "DECOY-RAN").exists(),
                         "the shipped installer must not start in a session")

    def test_the_ancestor_refusal_names_the_socket_it_found(self):
        """The refusal must tell the operator WHICH daemon it saw: a bare "refused"
        leaves a CI failure undiagnosable."""
        if self.report.exists():
            self.report.unlink()
        process = self.invoke("real", under_fake_supervisor=True)
        self.assertIn("refusing to run the real installer from under a live daemon",
                      process.stderr)
        self.assertIn("fixture-daemon.sock", process.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)
