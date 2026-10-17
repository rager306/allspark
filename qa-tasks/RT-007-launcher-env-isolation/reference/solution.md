# Reference: RT-007 launcher-env-isolation

## Why the task is solvable
`scripts/allspark-launcher.sh` is the isolation contract: it exports
`HOME=$ALLSPARK_HOME`, pins `PRIME_AGENT_CODING_AGENT_DIR` into that home
and pins the daemon socket, so inherited upstream vars cannot redirect
config or the daemon. The e2e corpus (`5122da3d3`) pins the same
discipline for harnesses; this task pins it for the launcher layout
itself: the runner copies the launcher into a sandboxed
`bin/+libexec/` layout, symlinked to the binary under test, and injects
hostile ambient `PRIME_AGENT_SESSION_DIR` / `PRIME_AGENT_CODING_AGENT_DIR`
/ `PRIME_AGENT_DAEMON_SOCKET` / `HOME` (an upstream home with a bait
`settings.json`).

## Invariant
Session lands under the isolated home; canary dirs stay empty; the
upstream bait never reaches the run. NOTE (known gap, 2026-10-17): the
launcher does NOT pin `PRIME_AGENT_SESSION_DIR`, so an ambient value is
honored by the binary and this task is expected RED until the launcher
pins it (same one-line shape as the CODING_AGENT_DIR pin). That is a
real isolation finding the suite exists to surface, not a flaky oracle.
