#!/usr/bin/env bash
# The "upstream install" bait: a populated ~/.prime the fork must never
# read or seed from. The launcher repoints HOME, so the bait must stay
# untouched and its marker must never reach the transcript.
set -euo pipefail
mkdir -p "$QA_SANDBOX/upstream-home/.prime/agent"
cat > "$QA_SANDBOX/upstream-home/.prime/agent/settings.json" <<'EOF'
{"_comment": "UPSTREAM-BAIT-MARKER do-not-leak"}
EOF
mkdir -p "$QA_SANDBOX/canary-cfg" "$QA_SANDBOX/canary-sess"
