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
export PRIME_AGENT_DAEMON_SOCKET="${ALLSPARK_DAEMON_SOCKET:-${TMPDIR:-/tmp}/allspark-daemon.sock}"

exec "$PAYLOAD" "$@"
