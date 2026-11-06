#!/bin/sh
# allspark -- isolated launcher for the Allspark fork of prime-agent.
# The fork runs in its own HOME so sessions, auth, config, logs, kernel venv,
# and the daemon socket never share state with a co-installed upstream
# prime-agent. All product paths derive from $HOME, so HOME is the only
# isolation knob. Set ALLSPARK_HOME to override the isolated home
# (default: $HOME/.allspark-home).

SELF_DIR="$(cd "$(dirname "$0")" && pwd)"

if [ -z "${ALLSPARK_HOME:-}" ]; then
    ALLSPARK_HOME="$HOME/.allspark-home"
fi

# NEVER NEST. `allspark` launched from inside an allspark session inherits that
# session's HOME (the launcher set it), so `$HOME/.allspark-home` becomes
# `…/.allspark-home/.allspark-home`: a second agent home with its own kernel venv,
# its own uv toolchain, and its own sessions, none of which the outer home can see
# (measured: 184 MB of it, one session, discovered 2026-11-06). Re-running the
# launcher must land in the SAME home, not a deeper copy of it.
#
# The check is on the RESOLVED path: `/root/.allspark-home/./`, a `..` segment, or
# a symlinked spelling all name the same directory, and comparing the raw strings
# would let each one through.
# Strip until the PARENT is no longer an allspark home: check the parent, not the
# path itself, or the first iteration breaks immediately (every nested path already
# ends in `/.allspark-home`). The resolution above makes `./`, `..` and symlinked
# spellings compare as the directory they name.
ALLSPARK_HOME="$(cd "$ALLSPARK_HOME" 2>/dev/null && pwd -P || printf '%s' "$ALLSPARK_HOME")"
while :; do
    parent="${ALLSPARK_HOME%/.allspark-home}"
    case "$parent" in
        */.allspark-home) ALLSPARK_HOME="$parent" ;;
        *) break ;;
    esac
done

PAYLOAD_DIR="$(cd "$SELF_DIR/.." && pwd)/libexec"
PAYLOAD="$PAYLOAD_DIR/prime-agent"

if [ ! -x "$PAYLOAD" ]; then
    # Fall back to the same directory as the launcher itself.
    PAYLOAD="$SELF_DIR/prime-agent"
fi

if [ ! -x "$PAYLOAD" ]; then
    echo "allspark: payload binary missing at $PAYLOAD" >&2
    exit 1
fi

mkdir -p "$ALLSPARK_HOME"

# No seeding from the upstream install by design: the fork starts with its
# own providers, settings and auth only. Nothing under $HOME/.prime is ever
# read, so upstream providers (their models.json) can never leak in.

export HOME="$ALLSPARK_HOME"

# Pin the agent state dir to the isolated home: an inherited upstream
# PRIME_AGENT_CODING_AGENT_DIR (launching allspark from inside a
# prime-agent session) would otherwise point the fork at the upstream
# config/models/auth.
export PRIME_AGENT_CODING_AGENT_DIR="$ALLSPARK_HOME/.prime/agent"

# Pin the sessions dir with the same discipline: an inherited
# PRIME_AGENT_SESSION_DIR must not redirect the fork's session storage into
# the upstream tree (found by the deviation suite's launcher-isolation task,
# RT-007).
export PRIME_AGENT_SESSION_DIR="$ALLSPARK_HOME/.prime/agent/sessions"

# Pin the daemon socket to the fork's own file so it cannot fight the
# upstream daemon (which uses the product default $TMPDIR/prime-agent-<uid>).
#
# UNCONDITIONAL, never `${VAR:-default}`: an upstream session exports
# PRIME_AGENT_DAEMON_SOCKET to its workers, so an inherited value would point
# the fork's daemon at the upstream one's socket. Measured 2026-10-23: launching
# the fork from inside an upstream session passed
# /tmp/prime-agent-rust-0/daemon.sock straight through, and the next
# `allspark update` stopped the upstream daemon (both supervisors received
# `installer-shutdown` 3.4s apart). ALLSPARK_DAEMON_SOCKET is the explicit
# override for a fork that really wants a different file.
# The socket lives in a PRIVATE per-user directory, beside the home and mode 0700, not
# directly in the shared temp dir: `${TMPDIR}/allspark-daemon.sock` is predictable, so any
# local user can bind it first and answer with the hello fields the client validates —
# every one of whom the peer supplies (review #17, CWE-290). A directory only this user can
# enter removes the surface; the client's SO_PEERCRED check (`pa_types::platform::
# transport`) is the second lock, for the case where the path was chosen some other way.
#
# The directory is created here, before the daemon sees the path, with 0700 as its MODE AT
# CREATION (a chmod after `mkdir` leaves a window in which the directory is traversable).
ALLSPARK_SOCKET_DIR="${ALLSPARK_DAEMON_SOCKET_DIR:-$ALLSPARK_HOME/run}"
if ! mkdir -p -m 700 "$ALLSPARK_SOCKET_DIR" 2>/dev/null; then
    echo "allspark: cannot create the socket directory $ALLSPARK_SOCKET_DIR" >&2
    exit 1
fi
# A directory that already existed keeps its mode, so it is CHECKED — but mode alone is not
# the property that matters: a 0755 directory owned by ANOTHER user is readable by us and
# writable only by them, which is exactly the impersonation window (they bind the socket, we
# connect to it). The check is ownership first, then the mode bits another user could write
# through (0700 and 0750 are ours alone; 0755 lets anyone traverse and read, which is a
# weaker but real exposure on a multi-user host, so it is refused too).
socket_dir_owner="$(stat -c %u "$ALLSPARK_SOCKET_DIR" 2>/dev/null || echo unknown)"
socket_dir_mode="$(stat -c %a "$ALLSPARK_SOCKET_DIR" 2>/dev/null || echo unknown)"
if [ "$socket_dir_owner" != "$(id -u)" ]; then
    echo "allspark: $ALLSPARK_SOCKET_DIR belongs to uid $socket_dir_owner, not $(id -u):" \
         "a socket another user owns is a socket another user answers on" >&2
    exit 1
fi
case "$socket_dir_mode" in
    700|750) : ;;
    *)
        echo "allspark: $ALLSPARK_SOCKET_DIR must be a private directory (mode 700 or 750)" \
             "but is $socket_dir_mode" >&2
        exit 1
        ;;
esac
export PRIME_AGENT_DAEMON_SOCKET="${ALLSPARK_DAEMON_SOCKET:-$ALLSPARK_SOCKET_DIR/daemon.sock}"

# THE FORK UPDATE CHANNEL: without it the update funnel refuses fork builds
# (the default endpoints belong to upstream prime-agent, and updating through
# them would replace this build with one that lacks the fork crates). The
# fork release publishes `stable`, `latest.json`, and the tarballs as release
# assets, so GitHub's latest/download URLs ARE the fork channel. The funnel
# fetches <base>/stable, <base>/latest.json, and <base>/<archive> from it.
# ALLSPARK_DOWNLOAD_BASE_URL is the explicit override (a self-hosted mirror).
export PRIME_AGENT_DOWNLOAD_BASE_URL="${ALLSPARK_DOWNLOAD_BASE_URL:-https://github.com/rager306/allspark/releases/latest/download}"

exec "$PAYLOAD" "$@"
