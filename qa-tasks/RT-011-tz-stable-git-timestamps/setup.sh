#!/usr/bin/env bash
# Three commits whose committer dates sit in unusual offsets (incl. the
# +05:45 Nepal offset), so any local-time formatting drifts between TZs.
set -euo pipefail
git init -q repo
cd repo
git config user.email qa@suite.local
git config user.name "QA Suite"
for d in "2026-01-02T10:00:00+05:45" "2026-03-15T23:30:00-03:00" "2026-07-30T01:15:00+09:00"; do
  GIT_COMMITTER_DATE="$d" GIT_AUTHOR_DATE="$d" git commit -q --allow-empty -m "commit at $d"
done
echo "repo ready"
