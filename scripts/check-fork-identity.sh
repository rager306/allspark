#!/bin/bash
# Fork identity guard (isolated-by-default scheme): the fork must not read
# upstream PRIME_AGENT_* / PI_* env vars or write upstream paths. Run after
# every upstream merge; any hit means a merge re-introduced a collision.
set -euo pipefail
cd "$(dirname "$0")/.."

fail=0
check() {
    local label="$1" pattern="$2"
    local hits
    hits=$(rg -n -e "$pattern" crates prime-agent-runtime install-rust.sh install.ps1 skills \
            -g '*.rs' -g '*.py' -g '*.toml' -g '*.sh' -g '*.ps1' 2>/dev/null \
        | grep -v 'docs/fork-identity.md' \
        | grep -v 'migrate_from_legacy_once\|legacy = home.join' \
        | grep -v 'PRIME_API_KEY' \
        | grep -v 'DO_NOT_TRACK' \
        | wc -l)
    if [ "$hits" -gt 0 ]; then
        echo "IDENTITY LEAK [$label]: $hits site(s)"
        rg -n -e "$pattern" crates prime-agent-runtime install-rust.sh install.ps1 skills \
            -g '*.rs' -g '*.py' -g '*.toml' -g '*.sh' -g '*.ps1' 2>/dev/null \
            | grep -v 'migrate_from_legacy_once\|legacy = home.join' \
            | grep -v 'PRIME_API_KEY' \
            | grep -v 'DO_NOT_TRACK' | head -5
        fail=1
    fi
}

check 'upstream-env-dir'    'PRIME_AGENT_CODING_AGENT_DIR'
check 'upstream-env-socket' 'PRIME_AGENT_DAEMON_SOCKET'
check 'upstream-env-sess'   'PRIME_AGENT_SESSION_DIR'
check 'upstream-env-pkg'    '"PI_PACKAGE_DIR"'
check 'config-dir'          '\.prime/agent'
check 'runtime-socket'      '/tmp/prime-agent-'
check 'install-share'       'share/prime-agent'

if [ "$fail" -eq 0 ]; then
    echo "identity guard: clean (isolated-by-default)"
else
    exit 1
fi
