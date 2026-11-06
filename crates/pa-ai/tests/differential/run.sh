#!/usr/bin/env bash
# Prime-inference differential verifier.
#
# 1. Replays the captured real SSE exchange with z-ai/glm-5.3-flash through a
#    local server.
# 2. Runs the TS reference provider (scratch copy of the TS repo with
#    node_modules) and the Rust port against the same bytes.
# 3. Fails if assistant text, usage accounting, or stop reasons differ.
#
# Usage: tests/differential/run.sh [ts-repo-scratch-dir]
#   ts-repo-scratch-dir defaults to /tmp/pa-ts (a writable copy of the TS repo
#   with node_modules installed).
#
# Usage: PA_DIFF_SELFTEST=1 tests/differential/run.sh
#   Checks the two isolation properties without needing a TS copy.

set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
CRATE_DIR="$HERE/../.."
REPO_DIR="$(cd "$CRATE_DIR/../.." && pwd)"
TS_SCRATCH="${1:-/tmp/pa-ts}"
# Port 0 lets the kernel choose. A fixed default meant two concurrent runs — a developer's
# and CI's, or two agents' — fought for the same port, and the loser's readiness check then
# saw the WINNER's server answering: both runs compared against one server, and one of them
# reported a mismatch that was a race. PA_DIFF_PORT still pins a port when a caller needs it.
PORT_REQUEST="${PA_DIFF_PORT:-0}"

# Reading the port the server REPORTED is the whole point: with a requested 0 the caller
# cannot know it any other way, and with a requested n it is still the server's answer.
port_of() {
  local log="$1" found=""
  for _ in $(seq 1 100); do
    found="$(sed -n 's/^REPLAY_PORT=\([0-9][0-9]*\)$/\1/p' "$log" | head -1)"
    if [ -n "$found" ]; then printf '%s' "$found"; return 0; fi
    sleep 0.05
  done
  return 1
}

# --- self-test (review #22) -------------------------------------------------------
# Two properties, both checkable on a machine with no TS checkout:
#   * a run gets its OWN scratch directory, so two runs cannot overwrite one baseline;
#   * the replay port is the one the SERVER reports, so two runs cannot share a server.
if [ "${PA_DIFF_SELFTEST:-}" = "1" ]; then
  SCRATCH_A="$(mktemp -u "$REPO_DIR/.tmp/differential-selftest-a-XXXXXX")"
  SCRATCH_B="$(mktemp -u "$REPO_DIR/.tmp/differential-selftest-b-XXXXXX")"
  mkdir -p "$SCRATCH_A" "$SCRATCH_B"
  python3 "$HERE/replay_server.py" 0 >"$SCRATCH_A/replay.log" 2>&1 &
  SERVER_A=$!
  python3 "$HERE/replay_server.py" 0 >"$SCRATCH_B/replay.log" 2>&1 &
  SERVER_B=$!
  trap 'kill $SERVER_A $SERVER_B 2>/dev/null || true; rm -rf "$SCRATCH_A" "$SCRATCH_B"' EXIT

  if [ "$SCRATCH_A" = "$SCRATCH_B" ]; then
    echo "SELFTEST FAIL: two runs share one scratch directory" >&2
    exit 1
  fi
  PORT_A="$(port_of "$SCRATCH_A/replay.log")" || {
    echo "SELFTEST FAIL: run A's server never reported a port" >&2
    cat "$SCRATCH_A/replay.log" >&2
    exit 1
  }
  PORT_B="$(port_of "$SCRATCH_B/replay.log")" || {
    echo "SELFTEST FAIL: run B's server never reported a port" >&2
    cat "$SCRATCH_B/replay.log" >&2
    exit 1
  }
  if [ "$PORT_A" = "$PORT_B" ]; then
    echo "SELFTEST FAIL: two servers report the same port $PORT_A" >&2
    exit 1
  fi
  echo "SELFTEST PASS: separate scratch dirs, separate ports ($PORT_A, $PORT_B)"
  exit 0
fi

if [ ! -d "$TS_SCRATCH/node_modules" ]; then
  echo "TS scratch dir $TS_SCRATCH has no node_modules; pass a runnable TS copy" >&2
  exit 1
fi

# This run's own scratch, inside the repository (`.tmp`, gitignored) rather than the OS temp
# dir: two runs used to write ONE shared baseline file, so one could read the other's bytes
# and report a differential mismatch that was a race.
RUN_DIR="$(mktemp -d "$REPO_DIR/.tmp/differential-XXXXXX")"

# Start the replay server and learn the port it actually bound.
python3 "$HERE/replay_server.py" "$PORT_REQUEST" >"$RUN_DIR/replay.log" 2>&1 &
REPLAY_PID=$!
trap 'kill $REPLAY_PID 2>/dev/null || true; rm -rf "$RUN_DIR"' EXIT
if ! PORT="$(port_of "$RUN_DIR/replay.log")"; then
  echo "replay server never reported a port:" >&2
  cat "$RUN_DIR/replay.log" >&2
  exit 1
fi
if ! kill -0 "$REPLAY_PID" 2>/dev/null; then
  echo "replay server exited before the comparison started:" >&2
  cat "$RUN_DIR/replay.log" >&2
  exit 1
fi

REPLAY_URL="http://127.0.0.1:$PORT/api/v1"
echo "replay server on port $PORT (scratch $RUN_DIR)"

# TS baseline: this run's own file, never a shared path.
TS_BASELINE="$RUN_DIR/ts.json"
cp "$HERE/ts_driver.ts" "$TS_SCRATCH/ts_driver.ts"
(cd "$TS_SCRATCH" && npx tsx ts_driver.ts "$REPLAY_URL") > "$TS_BASELINE"
echo "--- TS baseline written: $TS_BASELINE ---"

# Rust side.
PA_DIFF_REPLAY_URL="$REPLAY_URL" PA_DIFF_EXPECTED="$TS_BASELINE" \
  cargo test -p pa-ai --lib differential_prime_inference -- --ignored --nocapture

echo "DIFFERENTIAL PASS: Rust matches the TS reference for z-ai/glm-5.3-flash"
