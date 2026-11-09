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
    allowlist drops them) and refuses;
  * on a non-Linux host an over-long fixture TMPDIR makes the installer fall
    back to `/tmp/<name>-<uid>` — the ambient namespace — so the guard refuses a
    fixture socket that long instead of relying on TMPDIR to contain it;
  * the guard also refuses a socket path that resolves outside the fixture root.

Run: python3 scripts/release/test_channel_install_guard.py  (make release-gates)
"""
from __future__ import annotations

import importlib.util
import os
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
        # A live session's markers (measured on 2026-11-09).
        return {"PRIME_AGENT_INTERNAL_DAEMON_SUPERVISOR_SOCKET":
                "/tmp/prime-agent-rust-0/daemon.sock"}

    def outside_a_session(self):
        # These cases are about the OTHER arms, and this battery may itself run
        # inside a session (it does, under an agent): clear the markers so each
        # case exercises one rule, not the session refusal every time.
        return patch.dict(os.environ,
                          {"PRIME_AGENT_INTERNAL_DAEMON_WORKER": "",
                           "PRIME_AGENT_INTERNAL_DAEMON_SUPERVISOR_SOCKET": ""})

    def test_a_session_invocation_is_refused_before_an_environment_is_built(self):
        with patch.dict(os.environ, self.session_env()):
            with self.assertRaises(SystemExit) as caught:
                harness.isolated_environment(self.root / "user", "http://127.0.0.1:1")
        self.assertIn("inside a prime-agent session", str(caught.exception))

    def test_the_worker_marker_alone_is_enough(self):
        # The supervisor socket variable is one marker; the worker role is the
        # other, and a worker's parent has both — but neither is guaranteed.
        with patch.dict(os.environ, {"PRIME_AGENT_INTERNAL_DAEMON_WORKER": "1"}):
            with self.assertRaises(SystemExit):
                harness.guard_isolated_sockets(self.root, platform="Linux")

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
                harness.isolated_environment(self.root / "user-a", "http://127.0.0.1:1")
        source = (HERE / "test_channel_install.py").read_text()
        self.assertIn("isolated_environment(root / \"user\", base)", source,
                      "the channel harness must build its child env through the guard")


if __name__ == "__main__":
    unittest.main(verbosity=2)
