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

# First run: seed the isolated home with settings/models from the upstream
# install so config survives. auth.json is intentionally NOT copied (a
# rotating refresh token shared by two products invalidates the other's
# copy on first refresh); run allspark login for those providers.
if [ ! -f "$ALLSPARK_HOME/.prime/agent/settings.json" ] \
   && [ -f "$HOME/.prime/agent/settings.json" ]; then
    mkdir -p "$ALLSPARK_HOME/.prime/agent"
    cp "$HOME/.prime/agent/settings.json" "$ALLSPARK_HOME/.prime/agent/" 2>/dev/null
    cp "$HOME/.prime/agent/models.json" "$ALLSPARK_HOME/.prime/agent/" 2>/dev/null
    echo "allspark: seeded settings/models from ~/.prime/agent (auth: run allspark login)"
fi

export HOME="$ALLSPARK_HOME"

# Pin the daemon socket to the fork's own file so it cannot fight the
# upstream daemon (which uses the product default $TMPDIR/prime-agent-<uid>).
export PRIME_AGENT_DAEMON_SOCKET="${PRIME_AGENT_DAEMON_SOCKET:-${TMPDIR:-/tmp}/allspark-daemon.sock}"

exec "$PAYLOAD" "$@"
