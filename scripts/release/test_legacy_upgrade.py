#!/usr/bin/env python3
"""Exercise an unchanged TS installer against real candidate release archives.

This is an opt-in integration gate, not a mocked-binary unit test. Example:
  python3 scripts/release/test_legacy_upgrade.py \
    --archive /tmp/prime-agent-1.0.0-darwin-arm64.tar.gz \
    --installer-ref v0.9.8 --previous-archive /tmp/prime-agent-0.9.8-darwin-arm64.tar.gz

Supply one --installer-ref per verified Prime release, or --installer-file to
use exact bytes extracted from a published artifact. Do not assume every old
Git tag belongs to Prime Agent: the repository includes upstream project tags.
The JSON report distinguishes installer execution from CLI/TUI update coverage.
All download requests use a local HTTP server, and all installation/user paths
live in a temporary directory. Kernel bootstrap is deliberately disabled; that
requires its own real-runtime migration test.
"""
from __future__ import annotations

import argparse
import contextlib
import hashlib
import http.server
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import tempfile
import threading
import tarfile

REPO = Path(__file__).resolve().parents[2]
ARCHIVE_NAME = re.compile(r"prime-agent-(\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?)-(darwin|linux)-(arm64|x64)([^/]*)\.tar\.gz$")


def digest(path: Path) -> str:
    checksum = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            checksum.update(block)
    return checksum.hexdigest()


def archive_identity(path: Path) -> tuple[str, str]:
    match = ARCHIVE_NAME.fullmatch(path.name)
    if not match:
        raise ValueError(f"Expected a versioned native release archive: {path.name}")
    version, system, architecture, suffix = match.groups()
    return version, f"{system}-{architecture}{suffix}"


@contextlib.contextmanager
def release_server(root: Path):
    class Handler(http.server.SimpleHTTPRequestHandler):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, directory=str(root), **kwargs)

        def log_message(self, *_args):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}"
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


def digest_text(text: str) -> str:
    """The sha256 of a string: the fixture installer has no file of its own."""
    return hashlib.sha256(text.encode()).hexdigest()


def publish_installer(feed: Path, installer: Path, mode: str) -> None:
    """Publish the installer a run will download as `<feed>/install.sh`.

    `real` copies the shipped bytes: the release gate's subject. `fixture` writes
    `FIXTURE_INSTALLER_SCRIPT` INSTEAD, so a run can exercise channel → download →
    install → reinstall without executing the shipped script and without touching a
    socket. The name is the same either way, because that is the wire contract the
    installer download follows.
    """
    if mode not in INSTALLER_MODES:
        raise ValueError(f"unknown installer mode {mode!r}: expected one of {INSTALLER_MODES}")
    destination = feed / "install.sh"
    if mode == FIXTURE_INSTALLER:
        destination.write_text(FIXTURE_INSTALLER_SCRIPT)
    else:
        shutil.copyfile(installer, destination)


def publish_archive(feed: Path, archive: Path) -> Path:
    version, _ = archive_identity(archive)
    directory = feed / "releases" / f"v{version}"
    directory.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(archive, directory / archive.name)
    checksums = directory / "SHA256SUMS"
    checksums.write_text(f"{digest(archive)}  {archive.name}\n")
    return checksums


# The installer derives its stop candidates from TMPDIR: `prime-agent-<uid>`,
# `prime-agent-rust-<uid>`, `allspark-<uid>`, plus the launcher's `$ALLSPARK_HOME/run`.
# Above the unix sun_path limit it falls back to `/tmp` on non-Linux hosts — the
# AMBIENT namespace, where an operator's live daemon lives.
SOCKET_PATH_LIMIT = 103


def socket_candidates(root: Path) -> list[Path]:
    """The sockets the installer will probe, for a fixture rooted at `root`."""
    tmp = root / "tmp"
    uid = os.getuid()
    return [
        tmp / f"{name}-{uid}" / "daemon.sock"
        for name in ("prime-agent", "prime-agent-rust", "allspark")
    ] + [root / "home/.allspark-home/run/daemon.sock"]


@contextlib.contextmanager
def fixture_root(prefix: str):
    """A temporary fixture root SHORT enough that the installer never leaves it.

    Above the unix socket limit the installer re-anchors its sockets at
    `/tmp/<name>-<uid>` — the machine's shared namespace, where the operator's own
    daemon may be listening. A macOS runner's default temporary root
    (`/var/folders/…/T/prime-legacy-upgrade-XXXXXXXX`) is already long enough to
    trigger that, so the fixture is created in the SHORTEST temporary directory the
    host offers, and `guard_isolated_sockets` then proves it stayed inside.
    """
    for candidate in (None, "/tmp"):
        root = Path(tempfile.mkdtemp(prefix=prefix, dir=candidate))
        platform = os.uname().sysname
        longest = max(len(str(path).encode()) for path in socket_candidates(root))
        if platform == "Linux" or longest <= SOCKET_PATH_LIMIT:
            try:
                yield root
            finally:
                shutil.rmtree(root, ignore_errors=True)
            return
        shutil.rmtree(root, ignore_errors=True)
    raise SystemExit(
        f"no temporary directory is short enough for the fixture sockets "
        f"({SOCKET_PATH_LIMIT} bytes): the installer would probe the machine's own "
        "socket namespace. Set TMPDIR to a short path (e.g. /tmp).")


# WHICH installer a run may spawn.
#
# `real` copies the shipped bytes into the feed and is the release gate's subject
# (`release.yml`, outside every session). `fixture` stands in for that file when the
# probe is about the harness's OWN flow — channel publish, download, install,
# reinstall, user-state preservation — because the real installer's stop probe asks
# candidate daemons to shut down BY SOCKET PATH, which is the mechanism AGENTS.md's
# install rule exists to keep out of a live session. The fixture installs a fake
# launcher and never opens a socket.
#
# `auto` is the DEFAULT, and it is the only mode that decides anything: it runs the
# fixture exactly when this process lives inside a prime-agent session, and the
# shipped installer otherwise. The question is asked of the same two arms the guard
# uses — the session markers, then the process-tree walk that a scrubbed environment
# cannot remove ([`daemon_ancestor_socket`]) — so the default cannot be steered by the
# environment the caller happens to pass. A `real`/`fixture` request is obeyed as
# written, which is how the release gate names its subject and how a test names the
# refusal; `auto` is just the safe answer for a caller that did not think about it.
REAL_INSTALLER = "real"
FIXTURE_INSTALLER = "fixture"
AUTO_INSTALLER = "auto"
# The CONCRETE modes: what may be published into a feed.
INSTALLER_MODES = (REAL_INSTALLER, FIXTURE_INSTALLER)
# Every accepted `--installer-mode` value.
INSTALLER_MODE_CHOICES = (AUTO_INSTALLER, *INSTALLER_MODES)

# The stand-in installer written into the feed under `fixture`. It records the
# environment it was handed (so a probe can assert the isolation), installs a fake
# launcher that answers `--version` with the channel's version, and touches no
# socket, no daemon, and no path outside `$PRIME_AGENT_RUST_PREFIX`.
FIXTURE_INSTALLER_SCRIPT = r"""#!/bin/sh
set -eu
record="${FIXTURE_INSTALLER_RECORD:?the fixture installer needs a record dir}"
mkdir -p "$record" "${PRIME_AGENT_RUST_PREFIX}/bin"
env | sort > "$record/env.txt"
# The CHANNEL is real even here: the manifest must be served and must name the
# version this run expects, so a broken publish still fails the probe.
curl -fsSL "$PRIME_AGENT_DOWNLOAD_BASE_URL/latest.json" > "$record/latest.json"
python3 - "$record/latest.json" "$FIXTURE_INSTALLER_VERSION" <<'CHECK'
import json, sys
manifest = json.load(open(sys.argv[1]))
if manifest.get("version") != sys.argv[2]:
    raise SystemExit("channel manifest version %r != %r" % (manifest.get("version"), sys.argv[2]))
CHECK
cat > "${PRIME_AGENT_RUST_PREFIX}/bin/prime-agent" <<LAUNCHER
#!/bin/sh
case "\$1" in
  --version) printf '%s\n' "$FIXTURE_INSTALLER_VERSION" ;;
  *) : ;;
esac
exit 0
LAUNCHER
chmod +x "${PRIME_AGENT_RUST_PREFIX}/bin/prime-agent"
# The marker the probe asserts on: the bytes that ran were THIS fixture, not the
# shipped installer (which cannot run inside a session at all).
printf 'fixture-installer\n' > "$record/ran.txt"
"""

SHELL_INTERPRETERS = ("sh", "bash", "dash", "zsh")


def _argv_of(pid: int, platform: str) -> list[str]:
    """A live process's argv, best effort: /proc where it exists, else `ps`."""
    if platform == "Linux":
        try:
            return [
                part.decode("utf-8", "replace")
                for part in Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0")
                if part
            ]
        except OSError:
            return []
    try:
        out = subprocess.check_output(
            ["ps", "-o", "args=", "-p", str(pid)],
            stderr=subprocess.DEVNULL, universal_newlines=True, timeout=5)
    except (OSError, subprocess.SubprocessError):
        return []
    return out.split()


def _parent_of(pid: int, platform: str) -> int | None:
    if platform == "Linux":
        try:
            stat = Path(f"/proc/{pid}/stat").read_text(errors="replace")
            return int(stat.rsplit(")", 1)[-1].split()[1])
        except (OSError, ValueError, IndexError):
            return None
    try:
        out = subprocess.check_output(
            ["ps", "-o", "ppid=", "-p", str(pid)],
            stderr=subprocess.DEVNULL, universal_newlines=True, timeout=5).strip()
    except (OSError, subprocess.SubprocessError):
        return None
    return int(out) if out.isdigit() else None


def _daemon_socket_of(args: list[str]) -> str | None:
    """The socket a supervisor's argv names, or None when it is not a supervisor.

    The same two spellings the installer's own ancestor walk accepts
    (`--mode daemon --daemon-socket <socket>`, `supervisor --socket <socket>`), and
    the same executable gate: only a `prime-agent` argv counts, so an unrelated
    `sh --mode daemon` shell never produces a refusal.
    """
    if not args:
        return None
    named = os.path.basename(args[0]) in ("prime-agent", "prime-agent.exe")
    if not named and os.path.basename(args[0]) in SHELL_INTERPRETERS and len(args) > 1:
        named = os.path.basename(args[1]) in ("prime-agent", "prime-agent.exe")
        if named:
            args = args[1:]
    if not named:
        return None

    def after(flag: str) -> str | None:
        for index in range(len(args) - 1):
            if args[index] == flag:
                return args[index + 1]
        return None

    socket = after("--daemon-socket")
    if socket is None:
        socket = after("--socket")
    return socket if socket and socket.startswith("/") else None


def daemon_ancestor_socket(
    platform: str | None = None,
    ppid: int | None = None,
    argv_of=None,
    parent_of=None,
) -> str | None:
    """The socket of the daemon this process tree runs under, if any.

    Why this exists beside the environment markers: a scrubbed invocation
    (`env -i`, a service manager, a nested harness) drops the variables a daemon
    exports, and that is exactly how the 2026-10-21 incident reached a live
    supervisor. The process tree still carries the truth, so this walk is the
    marker-independent arm of the refusal — the mechanism `install-rust.sh`'s own
    probe uses. Best effort by construction: no ancestor, no `ps`, or a walk
    error answers None, which leaves the marker arms alone.
    """
    platform = platform or os.uname().sysname
    read_argv = argv_of or (lambda pid: _argv_of(pid, platform))
    read_parent = parent_of or (lambda pid: _parent_of(pid, platform))
    pid = os.getppid() if ppid is None else ppid
    seen: set[int] = set()
    while pid and pid > 1 and pid not in seen and len(seen) < 64:
        seen.add(pid)
        socket = _daemon_socket_of(read_argv(pid))
        if socket:
            return socket
        pid = read_parent(pid)
    return None


def session_marker() -> str | None:
    """The socket the session EXPORTED, or None; "the worker marker" when only that.

    The cheap arm: the daemon exports the supervisor socket and the worker role to
    its children, so an ordinary in-session invocation carries one. It is also the
    scrubbable arm — `env -i` drops both — which is why it is never the only one.
    """
    socket_path = os.environ.get("PRIME_AGENT_INTERNAL_DAEMON_SUPERVISOR_SOCKET")
    if socket_path:
        return socket_path
    if os.environ.get("PRIME_AGENT_INTERNAL_DAEMON_WORKER"):
        return os.environ.get("PRIME_AGENT_DAEMON_SOCKET") or "the worker marker"
    return None


def live_session_reason() -> str | None:
    """Why this run must not start the shipped installer, or None when it may.

    ONE definition of the question, read by both the refusal
    ([`guard_session`], [`guard_live_daemon_ancestor`]) and the mode default
    ([`resolve_installer_mode`]): a harness that refused one way and defaulted the
    other would be deciding twice, and only one of the two answers would be tested.
    """
    marker = session_marker()
    if marker:
        return f"inside a prime-agent session ({marker})"
    ancestor = daemon_ancestor_socket()
    if ancestor:
        return f"under a live daemon ({ancestor})"
    return None


def guard_session() -> None:
    """Refuse to run the REAL installer from inside a live agent session.

    AGENTS.md forbids it "in any form", and the reason is concrete: this harness
    spawns `install-rust.sh`, whose stop probe asks each candidate daemon to shut
    down BY SOCKET PATH — and the session that started the harness answers on
    one of them. The scrubbed fixture environment does not help; it narrows what
    the child sees, not what its stop probe stops.

    This arm reads the markers the session EXPORTS. They are the cheap signal and
    they are also the scrubbable one: `env -i` drops both, which is how the
    2026-10-21 incident reached a live supervisor. [`guard_live_daemon_ancestor`]
    is the arm that survives a scrubbed environment.
    """
    if session_marker():
        raise SystemExit(
            "refusing to run the real installer from inside a prime-agent session: "
            "it stops daemons by socket path, and this session's supervisor answers "
            "on one of them. Run this harness outside the session (CI does).")


def guard_live_daemon_ancestor() -> None:
    """Refuse when this process runs UNDER a daemon, whatever the environment says.

    The marker arm above is only as good as the environment it reads, and the
    harness is the caller that scrubs hardest. The process tree cannot be scrubbed:
    the walk names the supervisor this very run would be a child of, which is the
    one daemon whose socket probe is guaranteed to reach live work.
    """
    ancestor = daemon_ancestor_socket()
    if ancestor:
        raise SystemExit(
            "refusing to run the real installer from under a live daemon "
            f"({ancestor}): it stops daemons by socket path, and that is this "
            "session's own supervisor. Run this harness outside the session (CI does).")


def resolve_installer_mode(requested: str, *, session_reason: str | None) -> str:
    """The installer mode this run will actually use.

    `real` and `fixture` are obeyed as written: the release gate names its subject
    (`release.yml` runs outside every session) and a test names the refusal. `auto`
    — the default — answers with the fixture exactly when this process lives inside
    a prime-agent session, and with the shipped installer otherwise.

    `session_reason` is REQUIRED, like `isolated_environment`'s `live_installer`:
    whether this run may start a socket-probing installer is the one decision this
    module must never make silently, and the caller gets it from
    [`live_session_reason`] — one definition, so the refusal and the default can
    never answer the same question differently. It also makes the mapping a pure
    function in its test, which is why the guard battery does not need a daemon
    ancestor to pin it.

    Why the default must not be `real`: the shipped installer stops daemons BY
    SOCKET PATH, so a plain `python3 scripts/release/test_channel_install.py` inside
    a session used to abort (safe) and, before the process-tree walk existed, could
    reach a live supervisor (not safe). Defaulting to `auto` makes the safe path the
    one nobody has to know about, while a release runner still exercises the shipped
    script without passing a flag.
    """
    if requested in INSTALLER_MODES:
        return requested
    if requested != AUTO_INSTALLER:
        raise ValueError(
            f"unknown installer mode {requested!r}: expected one of {INSTALLER_MODE_CHOICES}")
    return FIXTURE_INSTALLER if session_reason else REAL_INSTALLER


def guard_isolated_sockets(root: Path, platform: str | None = None) -> None:
    """Refuse a fixture whose sockets the installer could not keep to itself.

    The installer stops daemons BY SOCKET PATH, and it derives those paths from
    TMPDIR — falling back to `/tmp/<name>-<uid>` on a non-Linux host when the
    fixture path is longer than the unix socket limit. That fallback IS the
    ambient namespace of the machine running this harness, so a long fixture path
    is refused rather than trusted to contain the child. A fixture socket that
    resolves outside the fixture root is refused for the same reason.

    Paths only: whether this RUN may start a socket-probing installer is the
    separate [`guard_session`] / [`guard_live_daemon_ancestor`] question, asked by
    `isolated_environment` where the mode is known.

    Fail-closed by design: there is no override here, and a harness that needs one
    is a harness whose sockets are not provably its own.
    """
    platform = platform or os.uname().sysname
    resolved_root = root.resolve()
    for candidate in socket_candidates(root):
        if not str(candidate.resolve()).startswith(str(resolved_root) + os.sep):
            raise SystemExit(f"the fixture socket {candidate} escapes the fixture root")
        if platform != "Linux" and len(str(candidate).encode()) > SOCKET_PATH_LIMIT:
            raise SystemExit(
                f"the fixture socket {candidate} is longer than {SOCKET_PATH_LIMIT} "
                f"bytes, so on {platform} the installer would fall back to /tmp — the "
                "AMBIENT socket namespace of the machine running this harness. Give the "
                "harness a shorter temporary root (TMPDIR).")


def isolated_environment(
    root: Path,
    base_url: str,
    layout: str = "custom",
    *,
    live_installer: bool,
) -> dict[str, str]:
    """The child environment for a harness run.

    `live_installer` is REQUIRED, not defaulted: it decides whether the shipped
    installer may start, and that is the one decision this module must never make
    silently. When it is true, a run under a live daemon (by exported marker, or by
    the process tree when the markers were scrubbed) is refused; a fixture
    installer opens no socket, so the fixture mode is the one that may run inside a
    session.
    """
    # An allowlist prevents inheriting credentials, daemon addresses, package
    # roots, update overrides, or user-specific runtime paths from the host.
    guard_isolated_sockets(root)
    if live_installer:
        guard_session()
        guard_live_daemon_ancestor()
    env = {name: os.environ[name] for name in ("PATH", "LANG", "LC_ALL", "SYSTEMROOT") if name in os.environ}
    for name, relative in {
        "HOME": "home", "TMPDIR": "tmp", "XDG_DATA_HOME": "data",
        "XDG_CONFIG_HOME": "config", "XDG_CACHE_HOME": "cache",
        "XDG_RUNTIME_DIR": "run", "PRIME_AGENT_CODING_AGENT_DIR": "home/.prime/agent",
        "PRIME_AGENT_INSTALL_DIR": "install root", "PRIME_AGENT_BIN_DIR": "public bin",
    }.items():
        path = root / relative
        path.mkdir(parents=True, exist_ok=True)
        env[name] = str(path)
    if layout == "standard":
        for name, relative in {"XDG_DATA_HOME": "home/.local/share", "PRIME_AGENT_INSTALL_DIR": "home/.local/share/prime-agent", "PRIME_AGENT_BIN_DIR": "home/.local/bin"}.items():
            path = root / relative
            path.mkdir(parents=True, exist_ok=True)
            env[name] = str(path)
    env["npm_config_prefix"] = str(root / "npm")
    env["npm_config_userconfig"] = str(root / "npmrc")
    env.update({
        "SHELL": "/bin/sh", "DO_NOT_TRACK": "1", "TERM": "dumb",
        "PRIME_AGENT_DOWNLOAD_BASE_URL": base_url,
        "PRIME_AGENT_ALLOW_INSECURE_HTTP_FOR_TESTS": "1",
        "PRIME_AGENT_INSTALL_METHOD": "binary", "PRIME_AGENT_INSTALLER_NONINTERACTIVE": "1",
        "PRIME_AGENT_INSTALLER_PLAIN": "1", "PRIME_AGENT_BOOTSTRAP_KERNEL_ON_INSTALL": "0",
        "PRIME_AGENT_INSTALL_LINK": "1", "PRIME_AGENT_PROBE_TIMEOUT_SECONDS": "30",
    })
    env["PATH"] = env["PRIME_AGENT_BIN_DIR"] + os.pathsep + env.get("PATH", "/usr/bin:/bin")
    return env


def execute(command: list[str], env: dict[str, str], expected_success: bool = True) -> dict:
    process = subprocess.Popen(command, env=env, cwd=env["HOME"], stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, text=True, start_new_session=True)
    try:
        stdout, stderr = process.communicate(timeout=180)
    except subprocess.TimeoutExpired:
        # Installer descendants must not outlive the temporary installation.
        os.killpg(process.pid, signal.SIGKILL)
        stdout, stderr = process.communicate()
        raise RuntimeError(json.dumps({"command": command, "error": "timed out after 180s", "stdout": stdout, "stderr": stderr}))
    evidence = {"command": command, "exit_code": process.returncode, "stdout": stdout, "stderr": stderr}
    if (process.returncode == 0) != expected_success:
        raise RuntimeError(json.dumps(evidence, indent=2))
    return evidence


def exercise(installer: Path, archive: Path, previous: Path | None, root: Path, entrypoint: str, next_archive: Path | None = None, layout: str = "custom") -> dict:
    version, platform = archive_identity(archive)
    feed = root / "feed"
    checksums = publish_archive(feed, archive)
    if previous:
        previous_version, previous_platform = archive_identity(previous)
        if previous_platform != platform or previous_version == version:
            raise ValueError("Previous archive must have the same platform and a different version")
        publish_archive(feed, previous)
    (feed / "latest.json").write_text(json.dumps({
        "version": version, "binaries": [{"platform": platform, "file": archive.name, "sha256": digest(archive)}],
    }))
    if next_archive:
        publish_archive(feed, next_archive)
        with tarfile.open(next_archive) as payload:
            script = payload.extractfile("install.sh")
            if script is None:
                raise ValueError("Next Rust archive lacks its installer")
            (feed / "install.sh").write_bytes(script.read())
    steps = []
    with release_server(feed) as base_url:
        env = isolated_environment(root / "user", base_url, layout, live_installer=True)
        state = Path(env["PRIME_AGENT_CODING_AGENT_DIR"])
        # Opaque content witnesses that installation never edits existing state.
        witnesses = {
            "settings.json": '{"migrationWitness":"settings"}\n',
            "auth.json": '{"migrationWitness":"credentials"}\n',
            "sessions/migration/session.jsonl": '{"migrationWitness":"session"}\n',
            "skills/migration/SKILL.md": "# Migration witness\n",
        }
        for relative, contents in witnesses.items():
            path = state / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(contents)
        public = Path(env["PRIME_AGENT_BIN_DIR"]) / "prime-agent"
        active = Path(env["PRIME_AGENT_INSTALL_DIR"]) / "bin/prime-agent"
        if previous:
            steps.append(execute(["sh", str(installer), previous_version], env))
            probe = execute([str(public), "--version"], env)
            assert probe["stdout"].strip() == previous_version, probe
            steps.append(probe)
            old_target = os.readlink(active)
            old_binary_hash = digest(active.resolve())
            # A corrupted candidate must not change the active TS installation.
            good_checksums = checksums.read_text()
            checksums.write_text(f"{'0' * 64}  {archive.name}\n")
            steps.append(execute(["sh", str(installer), version], env, expected_success=False))
            assert os.readlink(active) == old_target, "Failed upgrade changed active target"
            assert digest(active.resolve()) == old_binary_hash, "Failed upgrade changed active executable"
            checksums.write_text(good_checksums)
            env["PRIME_AGENT_EXPECTED_CURRENT"] = old_target
            env["PRIME_AGENT_EXPECTED_SHA256"] = digest(archive)
        update_command = [str(public), "update"] if entrypoint == "cli" else ["sh", str(installer), version]
        steps.append(execute(update_command, env))
        probe = execute([str(public), "--version"], env)
        assert probe["stdout"].strip() == version, probe
        steps.append(probe)
        steps.append(execute([str(public), "--help"], env))
        assert digest(active.resolve()) == digest(public.resolve()), "Public command does not resolve to activated executable"
        for relative, contents in witnesses.items():
            assert (state / relative).read_text() == contents, f"User state changed: {relative}"
        if previous:
            retained = active.with_name("previous")
            assert os.readlink(retained) == old_target, "Previous release not retained for recovery"
        if next_archive:
            next_version, next_platform = archive_identity(next_archive)
            assert next_platform == platform, "Second Rust archive uses another platform"
            (feed / "latest.json").write_text(json.dumps({"version": next_version, "binaries": [{"platform": platform, "file": next_archive.name, "sha256": digest(next_archive)}]}))
            (feed / "stable").write_text(next_version + "\n")
            env.pop("PRIME_AGENT_EXPECTED_CURRENT", None)
            env.pop("PRIME_AGENT_EXPECTED_SHA256", None)
            env["PRIME_AGENT_RUST_INSTALLER_URL"] = base_url + "/install.sh"
            env["PRIME_AGENT_ALLOW_HTTP"] = "1"
            steps.append(execute([str(public), "update"], env))
            probe = execute([str(public), "--version"], env)
            steps.append(probe)
            if probe["stdout"].strip() != next_version:
                raise RuntimeError(json.dumps({"error": "Second update did not advance the original public command", "expected_version": next_version, "steps": steps}))
            for relative, contents in witnesses.items():
                assert (state / relative).read_text() == contents, f"Second update changed user state: {relative}"
            # The managed coordinator can start a successor even when there
            # was no prior daemon. Stop that isolated successor before cleanup.
            steps.append(execute([str(public), "shutdown", "--force"], env))
    return {"status": "passed", "layout": layout,
 "platform": platform, "candidate_version": version,
            "previous_version": archive_identity(previous)[0] if previous else None,
            "coverage": ["real released CLI update" if entrypoint == "cli" else "unchanged shipped installer", "version/help probes", "paths with spaces", "user state bytes"]
            + (["checksum failure preserves active release", "previous release retention"] if previous else []) + (["second Rust update through same public command"] if next_archive else []),
            "not_covered": ([] if entrypoint == "cli" else ["CLI update command"]) + ["TUI /update", "daemon handoff", "session continuation", "kernel bootstrap"] + ([] if next_archive else ["second Rust update"]),
            "steps": steps}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--archive", required=True, type=Path)
    parser.add_argument("--previous-archive", type=Path)
    parser.add_argument("--next-archive", type=Path, help="Newer Rust artifact for a second update through the migrated command")
    parser.add_argument("--layout", choices=("custom", "standard"), default="custom")
    parser.add_argument("--installer-ref", action="append", default=[], help="Verified release tag/commit containing install.sh")
    parser.add_argument("--installer-file", action="append", type=Path, default=[], help="Installer extracted unchanged from a released artifact")
    parser.add_argument("--entrypoint", choices=("installer", "cli"), default="installer")
    parser.add_argument("--report", type=Path, help="Write machine-readable evidence (including failures)")
    args = parser.parse_args()
    if not args.installer_ref and not args.installer_file:
        parser.error("Supply at least one --installer-ref or --installer-file")
    if args.entrypoint == "cli" and not args.previous_archive:
        parser.error("--entrypoint cli requires --previous-archive")
    archives = [args.archive.resolve()] + ([args.previous_archive.resolve()] if args.previous_archive else [])
    if args.next_archive:
        archives.append(args.next_archive.resolve())
    report = {"archives": [{"path": str(path), "sha256": digest(path)} for path in archives], "runs": []}
    failed = False
    with fixture_root("prime-legacy-upgrade-") as directory:
        installers = []
        for index, ref in enumerate(args.installer_ref):
            content = subprocess.check_output(["git", "show", f"{ref}:install.sh"], cwd=REPO)
            commit = subprocess.check_output(["git", "rev-parse", f"{ref}^{{commit}}"], cwd=REPO, text=True).strip()
            installer = directory / f"installer-{index}.sh"
            installer.write_bytes(content)
            installers.append((installer, {"ref": ref, "commit": commit}))
        installers.extend((path.resolve(), {"file": str(path.resolve())}) for path in args.installer_file)
        for index, (installer, source) in enumerate(installers):
            entry = {"installer": {**source, "sha256": digest(installer)}}
            try:
                entry.update(exercise(installer, archives[0], args.previous_archive.resolve() if args.previous_archive else None, directory / f"case-{index}", args.entrypoint, args.next_archive.resolve() if args.next_archive else None, args.layout))
            except (AssertionError, OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
                entry.update(status="failed", error=str(error))
                failed = True
            report["runs"].append(entry)
    rendered = json.dumps(report, indent=2)
    if args.report:
        args.report.write_text(rendered + "\n")
    print(rendered)
    return int(failed)


if __name__ == "__main__":
    raise SystemExit(main())
