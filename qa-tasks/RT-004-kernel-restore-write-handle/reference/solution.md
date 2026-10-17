# Reference: RT-004 kernel-restore-write-handle

## Scenario
1. Cell 1 opens `out.dat` in `'wb'` mode, writes 1700 bytes, flushes, keeps
   the handle open in the kernel namespace.
2. Cell 2 sleeps 2.5s (so the debounced snapshot save after cell 1 lands
   WITH the handle in it), then kills the kernel (`os._exit(13)`): the
   kernel manager repairs (respawn + snapshot restore).
3. Cell 3 reads `out.dat` back and prints CONTENT-LEN.

## Why the task is solvable
`prime-agent-runtime/src/rlm/repl.py` neutralizes write-mode handles on
snapshot save/restore (fix `abe2a419b`): a `'w'/'a'/'x'/'+'-mode` handle
is refused at unpickle time and mapped to `/dev/null`, so restore can
never re-open the target with O_TRUNC. Red-proof (executed 2026-10-17,
`rlm._snapshot_state`/`_restore_state` driven directly, pre-fix vs fixed
runtime):

* pre-fix runtime (`abe2a419b^`): restore reopens `out.dat` `'wb'` ->
  **file truncated to 0** -> oracle fails (task RED).
* fixed runtime: restore reports `neutralized: h (wb)` -> file intact at
  1700 bytes -> task GREEN.

## Runtime deployment note
The kernel interpreter defaults to the machine's
`~/.prime/agent/kernel-venv`; the product upgrades its embedded
`prime-agent-runtime` in that venv when the binary is newer (observed
live on 2026-10-17: the first trial upgraded repl.py to the tree's
fixed version). In CI each sandbox may pin its own kernel venv
(`QA_KERNEL_PYTHON`) to keep trials hermetic; the manifest records the
interpreter path so a red result can be attributed to a stale runtime.
