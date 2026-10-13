#!/usr/bin/env sh
# Fork identity guard: after every upstream merge, catch upstream names that
# would collide with a co-installed `prime-agent`. The fork must share NO
# namespace with upstream (env vars, sockets, install paths, meta keys).
# Allowlist pins the deliberate upstream references (migration source,
# PRIME_API_KEY, DO_NOT_TRACK, and the wire-compat notes).
set -eu
cd "$(dirname "$0")/.."

allow() {
    grep -v 'pa-types/src/identity.rs' \
        | grep -v 'crates/pa-cli/src/lib.rs' \
        | grep -v 'crates/pa-core/src/auth/prime_inference.rs' \
        | grep -v 'crates/pa-daemon/src/hold_refusal.rs' \
        | grep -v 'install-rust.sh:.*protocol_name' \
        | grep -v 'PRIME_API_KEY' \
        | grep -v 'DO_NOT_TRACK' \
        | grep -v 'testdata/acp/'
}

fail=0
check() {
    label="$1"; pattern="$2"
    hits=$(grep -rIn -- "$pattern" crates prime-agent-runtime install-rust.sh install.ps1 skills --include='*.rs' --include='*.py' --include='*.toml' --include='*.sh' --include='*.ps1' --include='*.md' 2>/dev/null | allow | wc -l)
    if [ "$hits" -gt 0 ]; then
        echo "IDENTITY LEAK [$label]: $hits site(s)"
        grep -rIn -- "$pattern" crates prime-agent-runtime install-rust.sh install.ps1 skills --include='*.rs' --include='*.py' --include='*.toml' --include='*.sh' --include='*.ps1' --include='*.md' 2>/dev/null | allow | head -5
        fail=1
    fi
}

check 'env'          'PRIME_AGENT_'
check 'env-pi'       '"PI_[A-Z]'
check 'env-scripts'  'PRIME_AGENT_'
check 'config-dir'   '\.prime/agent'
check 'socket'       '/tmp/prime-agent-'
check 'install-path' 'share/prime-agent'
check 'install-marker' 'prime-agent-install'
check 'meta-key'     'ai\.primeintellect\.prime-agent'
check 'trace-id'     '"prime-agent-traces"'
check 'protocol'     '"prime-agent\.daemon"'
check 'bin-name'     'name = "prime-agent"'
check 'cargo-bin-env' 'CARGO_BIN_EXE_prime-agent'

if [ "$fail" -eq 0 ]; then
    echo "identity guard: clean"
else
    exit 1
fi
