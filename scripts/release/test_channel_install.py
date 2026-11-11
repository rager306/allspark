#!/usr/bin/env python3
"""Exercise the current curl installer and reinstall against a local channel.

Three installer modes, because the probe has two jobs:

  * `auto` (the DEFAULT) asks where this run lives and answers with one of the
    two concrete modes: `fixture` inside a prime-agent session (by exported
    marker, or by the process tree when the markers were scrubbed), `real`
    otherwise. So the safe path is the one nobody has to know about: a bare
    `python3 scripts/release/test_channel_install.py` inside a session runs the
    stand-in, and the same command on a release runner runs the shipped script.
    A tree it cannot READ takes the same safe answer and then FAILS the run
    instead of reporting it: see [`resolved_on_doubt`].
  * `real` runs the SHIPPED installer — the release gate's subject, on a release
    runner with no session to protect. It is fail-closed by construction:
    `isolated_environment` refuses when the run is under a live daemon (the
    process-tree walk, which a scrubbed environment cannot remove) or when its
    fixture sockets could leave the fixture root.
  * `fixture` publishes a stand-in installer instead and asserts the CHANNEL flow
    — publish, download, install, reinstall, user state — without executing the
    shipped script at all.

`release.yml` passes no `--installer-mode`, so it runs `auto` on a runner with no
daemon ancestor: the release gate still exercises the shipped installer end to end,
and a test holds that the resolution really answers `real` there. A run whose
ancestry cannot be READ is the one case where the safe answer and the honest report
disagree: it refuses the shipped installer, and it also refuses to report the
stand-in as a passing install (`installer_mode_doubt`, exit 1).

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
    INSTALLER_MODE_CHOICES,
    REAL_INSTALLER,
    AUTO_INSTALLER,
    archive_identity,
    digest,
    digest_text,
    execute,
    fixture_root,
    isolated_environment,
    live_session_reason,
    session_evidence,
    publish_installer,
    publish_archive,
    release_server,
    resolve_installer_mode,
)


def build_parser() -> argparse.ArgumentParser:
    """The CLI's parser, exposed so a test can read the mode DEFAULT itself.

    The default is load-bearing beyond the flag: `release.yml` passes no
    `--installer-mode`, so whatever this default says decides whether the release
    gate exercises the shipped installer or a stand-in.
    """
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", required=True, type=Path)
    parser.add_argument("--installer", required=True, type=Path)
    parser.add_argument("--report", required=True, type=Path)
    parser.add_argument("--installer-mode", choices=INSTALLER_MODE_CHOICES,
                        default=AUTO_INSTALLER,
                        help="auto (default): the shipped installer outside a live session, "
                             "the stand-in inside one (the process tree decides); real: "
                             "always the shipped installer (release gate); fixture: always "
                             "the stand-in, which installs a fake launcher and opens no socket")
    return parser


def resolved_on_doubt(mode: str, requested: str, session_detected: bool) -> bool:
    """Whether `auto` answered with the stand-in because it could not TELL.

    The three inputs are the whole decision, which is why it is a function: `auto`
    resolving to the fixture is normal and good when a session was actually seen, and
    it is a doubt with no evidence when one was not. A run in the second state must
    not report a passing install — a release gate that quietly substitutes a
    stand-in it chose out of doubt has tested nothing and said everything went well.
    """
    return (mode == FIXTURE_INSTALLER and requested == AUTO_INSTALLER
            and not session_detected)


def main() -> int:
    args = build_parser().parse_args()
    # Only `auto` asks the question: an explicit mode is obeyed as written, and
    # the guard for a `real` run is asked again where it matters
    # (`isolated_environment`, from the resolved mode).
    session_reason, session_detected = (
        session_evidence() if args.installer_mode == AUTO_INSTALLER else (None, False))
    mode = resolve_installer_mode(args.installer_mode, session_reason=session_reason)
    version, platform = archive_identity(args.archive)
    report = {"version": version, "platform": platform, "archive_sha256": digest(args.archive),
              "installer_mode": mode, "requested_installer_mode": args.installer_mode,
              "steps": []}
    if resolved_on_doubt(mode, args.installer_mode, session_detected):
        # DOUBT IS NOT A PASS WITH A STAND-IN. `auto` picked the fixture because this
        # run's ancestry could not be read — the fail-closed answer for safety — but
        # a release gate that quietly reports green for a stand-in install it chose
        # out of doubt has tested nothing and said everything went well. So the run
        # stops here and names the doubt: this is the case where the safe default
        # and the honest result disagree, and the operator has to break the tie.
        report["installer_mode_reason"] = session_reason
        report["installer_mode_doubt"] = True
        report.update(
            status="failed",
            error="the installer mode could not be resolved: this run's process ancestry "
                  "is unreadable, so it cannot be shown to be outside a live daemon (the "
                  "shipped installer would be refused) and the stand-in must not be "
                  "reported as a passing install. Run where the process tree is readable "
                  "(the release runner is), or name --installer-mode explicitly.")
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps({key: value for key, value in report.items() if key != "steps"}))
        return 1
    if mode == REAL_INSTALLER:
        report["installer_sha256"] = digest(args.installer)
    else:
        report["installer_sha256"] = digest_text(FIXTURE_INSTALLER_SCRIPT)
    if args.installer_mode == AUTO_INSTALLER:
        # Why the default chose what it chose: the operator reads this instead of
        # wondering whether the shipped installer was skipped by accident.
        report["installer_mode_reason"] = session_reason
    try:
        # A SHORT fixture root: the installer re-anchors its sockets at /tmp when
        # TMPDIR is longer than the unix socket limit, and that is the machine's
        # own namespace (see `fixture_root` and `guard_isolated_sockets`).
        with fixture_root("prime-channel-install-") as root:
            feed = root / "feed"
            publish_archive(feed, args.archive)
            publish_installer(feed, args.installer, mode)
            (feed / "stable").write_text(version + "\n")
            (feed / "latest.json").write_text(json.dumps({"version": version, "binaries": [
                {"platform": platform, "file": args.archive.name, "sha256": digest(args.archive)}]}))
            with release_server(feed) as base:
                env = isolated_environment(root / "user", base,
                                           live_installer=mode == REAL_INSTALLER)
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
                if mode == FIXTURE_INSTALLER:
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
