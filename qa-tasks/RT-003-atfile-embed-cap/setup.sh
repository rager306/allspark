#!/usr/bin/env bash
# big.log: 4000 lines, ~2.7MB. Line 4000 carries a tail marker that must
# never reach the prompt (the cap embeds only the first 256KiB).
set -euo pipefail
{
  for i in $(seq 1 3999); do
    printf 'line-%06d %s\n' "$i" "$(head -c 660 /dev/zero | tr '\0' 'x')"
  done
  printf 'line-040000 RT003-TAIL-MARKER-39499\n'
} > big.log
