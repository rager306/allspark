#!/bin/bash
# Fork identity guard: verifies that critical isolation markers survived an
# upstream merge. Does NOT flag upstream names in code (that is the B+ scheme:
# co-existence by HOME isolation, not by renaming every string).
set -euo pipefail
cd "$(dirname "$0")/.."

fail=0

# 1. Fork-only files must exist and carry the FORK-ONLY marker.
for f in crates/pa-ai/src/models_fork.rs docs/fork-identity.md; do
    if [ ! -f "$f" ]; then
        echo "MISSING fork file: $f"
        fail=1
    fi
done
if [ -f crates/pa-ai/src/models_fork.rs ] && ! grep -q 'FORK-ONLY' crates/pa-ai/src/models_fork.rs; then
    echo "MISSING FORK-ONLY marker in crates/pa-ai/src/models_fork.rs"
    fail=1
fi

# 2. The fork's runtime dir must not use the upstream name.
if grep -q 'prime-agent-{uid}' crates/pa-daemon/src/platform/paths.rs 2>/dev/null; then
    echo "REGRESSION: socket_dir uses prime-agent-{uid} (must be allspark-{uid})"
    fail=1
fi

# 3. The dist profile must exist.
if ! grep -q 'profile.dist' Cargo.toml 2>/dev/null; then
    echo "MISSING [profile.dist] in Cargo.toml"
    fail=1
fi

# 4. The migration must skip auth.json.
if grep -q 'for name in \["settings.json", "auth.json"' crates/pa-cli/src/lib.rs 2>/dev/null; then
    echo "REGRESSION: migration copies auth.json (rotating refresh)"
    fail=1
fi

if [ "$fail" -eq 0 ]; then
    echo "fork identity: clean"
else
    exit 1
fi
