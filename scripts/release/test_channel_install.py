#!/usr/bin/env python3
"""Exercise the current curl installer and reinstall against a local channel.

Two installer modes, because the probe has two jobs:

  * `real` (the default) runs the SHIPPED installer — the release gate's subject,
    on a release runner with no session to protect. It is fail-closed by
    construction: `isolated_environment` refuses when the run is under a live
    daemon (the process-tree walk, which a scrubbed environment cannot remove) or
    when its fixture sockets could leave the fixture root.
  * `fixture` publishes a stand-in installer instead and asserts the CHANNEL flow
    — publish, download, install, reinstall, user state — without executing the
    shipped script at all. That is what a probe inside an agent session (and the
    harness's own CI battery) runs, because the shipped installer stops daemons by
    socket path and AGENTS.md forbids starting it from a session.

No provider requests are made either way.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path

from test_legacy_upgrade import (
    FIXTURE_INSTALLER,
    FIXTURE_INSTALLER_SCRIPT,
    INSTALLER_MODES,
    REAL_INSTALLER,
    archive_identity,
    digest,
    digest_text,
    execute,
    fixture_root,
    isolated_environment,
    publish_installer,
    publish_archive,
    release_server,
)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", required=True, type=Path)
    parser.add_argument("--installer", required=True, type=Path)
    parser.add_argument("--report", required=True, type=Path)
    parser.add_argument("--installer-mode", choices=INSTALLER_MODES, default=REAL_INSTALLER,
                        help="real: the shipped installer (release gate, outside sessions); "
                             "fixture: a stand-in that installs a fake launcher and opens no "
                             "socket")
    args = parser.parse_args()
    version, platform = archive_identity(args.archive)
    report = {"version": version, "platform": platform, "archive_sha256": digest(args.archive),
              "installer_mode": args.installer_mode, "steps": []}
    if args.installer_mode == REAL_INSTALLER:
        report["installer_sha256"] = digest(args.installer)
    else:
        report["installer_sha256"] = digest_text(FIXTURE_INSTALLER_SCRIPT)
    try:
        # A SHORT fixture root: the installer re-anchors its sockets at /tmp when
        # TMPDIR is longer than the unix socket limit, and that is the machine's
        # own namespace (see `fixture_root` and `guard_isolated_sockets`).
        with fixture_root("prime-channel-install-") as root:
            feed = root / "feed"
            publish_archive(feed, args.archive)
            publish_installer(feed, args.installer, args.installer_mode)
            (feed / "stable").write_text(version + "\n")
            (feed / "latest.json").write_text(json.dumps({"version": version, "binaries": [
                {"platform": platform, "file": args.archive.name, "sha256": digest(args.archive)}]}))
            with release_server(feed) as base:
                env = isolated_environment(root / "user", base,
                                           live_installer=args.installer_mode == REAL_INSTALLER)
                prefix = root / "user" / "prefix with spaces"
                env.update(PRIME_AGENT_RUST_PREFIX=str(prefix), PRIME_AGENT_ALLOW_HTTP="1")
                # The fixture installer's evidence outlives the fixture root when
                # the caller names a place for it (the guard battery inspects the
                # environment it was handed); otherwise it lives with the fixture.
                record = Path(os.environ.get("FIXTURE_INSTALLER_RECORD") or root / "installer-record")
                env.update(FIXTURE_INSTALLER_RECORD=str(record),
                           FIXTURE_INSTALLER_VERSION=version)
                state = Path(env["PRIME_AGENT_CODING_AGENT_DIR"])
                witnesses = {"settings.json": '{}\n', "auth.json": '{}\n',
                             "sessions/preserved.jsonl": '{"migrationWitness":"saved session"}\n'}
                for name, content in witnesses.items():
                    path = state / name
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text(content)
                command = ["bash", "-o", "pipefail", "-c", 'curl -fsSL "$PRIME_AGENT_DOWNLOAD_BASE_URL/install.sh" | sh']
                for _ in range(2):
                    report["steps"].append(execute(command, env))
                    probe = execute([str(prefix / "bin/prime-agent"), "--version"], env)
                    report["steps"].append(probe)
                    if probe["stdout"].strip() != version:
                        raise ValueError("Installed launcher did not report channel version")
                    report["steps"].append(execute([str(prefix / "bin/prime-agent"), "--help"], env))
                    for name, content in witnesses.items():
                        if (state / name).read_text() != content:
                            raise ValueError(f"Installer changed user state: {name}")
                if args.installer_mode == FIXTURE_INSTALLER:
                    # The claim the fixture mode exists to make: the bytes that ran
                    # were the stand-in, not the shipped installer. A regression that
                    # put the real file back would fail here, not on a live socket.
                    ran = record / "ran.txt"
                    if not ran.is_file() or ran.read_text().strip() != "fixture-installer":
                        raise ValueError("the fixture installer did not run; the shipped one did")
                    manifest = json.loads((record / "latest.json").read_text())
                    if manifest.get("version") != version:
                        raise ValueError("the channel manifest the fixture saw is not this version")
                    report["fixture_installer_ran"] = True
                    report["installer_record"] = str(record)
                report.update(status="passed", fresh_install=True, reinstall=True, user_state_preserved=True)
    except Exception as error:
        report.update(status="failed", error=str(error))
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({key: value for key, value in report.items() if key != "steps"}))
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
