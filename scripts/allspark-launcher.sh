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

# Pin the daemon socket to the fork's own file so it cannot fight the
# upstream daemon (which uses the product default $TMPDIR/prime-agent-<uid>).
export PRIME_AGENT_DAEMON_SOCKET="${PRIME_AGENT_DAEMON_SOCKET:-${TMPDIR:-/tmp}/allspark-daemon.sock}"

exec "$PAYLOAD" "$@"
