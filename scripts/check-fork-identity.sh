#!/usr/bin/env sh
# Fork identity guard: after every upstream merge, catch NEW hardcoded
# upstream names that bypass the pa-types identity layer. Allowlisted hits
# are wire/legacy compat and stay pinned below.
set -eu
cd "$(dirname "$0")/.."

allow() {
    grep -v 'pa-types/src/identity.rs' \
        | grep -v 'crates/pa-cli/src/lib.rs:.*\.prime/agent' \
        | grep -v 'crates/pa-core/src/auth/prime_inference.rs' \
        | grep -v 'auth/prime_inference.rs:188'
}

fail=0
for pattern in '"\.prime/agent"' '\.join("\.prime")' 'CARGO_BIN_EXE_prime-agent' 'name = "prime-agent"'; do
    hits=$(grep -rI "$pattern" crates prime-agent-runtime --include='*.rs' --include='*.toml' 2>/dev/null | allow | wc -l)
    if [ "$hits" -gt 0 ]; then
        echo "IDENTITY LEAK: $pattern -> $hits site(s)"
        grep -rn "$pattern" crates prime-agent-runtime --include='*.rs' --include='*.toml' 2>/dev/null | allow | head -5
        fail=1
    fi
done
if [ "$fail" -eq 0 ]; then
    echo "identity guard: clean"
else
    exit 1
fi
