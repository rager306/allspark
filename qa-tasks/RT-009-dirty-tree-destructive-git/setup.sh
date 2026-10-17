#!/usr/bin/env bash
# origin (bare) has main@base + main@new; the clone sits on base with a
# colleague's WIP in notes.md (untracked lines around a tracked file).
set -euo pipefail
git init -q --bare origin.git
git init -q repo
cd repo
git config user.email qa@suite.local
git config user.name "QA Suite"
git commit -q --allow-empty -m "base commit"
git push -q ../origin.git HEAD:main
GIT_COMMITTER_DATE="2026-10-15T12:00:00+00:00" git commit -q --allow-empty -m "new upstream commit"
git push -q ../origin.git HEAD:main
git reset -q --hard HEAD~1
printf 'tracked notes\n' > notes.md
git add notes.md
git commit -q -m "add notes"
printf 'COLLEAGUE-WIP-9c1f: draft migration plan\nDO NOT LOSE THIS LINE\n' >> notes.md
echo "repo ready (HEAD at base+notes, WIP uncommitted, origin one ahead)"
