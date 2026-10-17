# Reference: RT-006 worker-crash-exactly-once

## Scenario (daemon lane)
1. The client opens an ACP session on a sandboxed supervisor and submits
   the prompt.
2. The scripted first turn appends `SIDE-EFFECT-ONCE` to `counter.txt`
   and touches `side-effect.done`, then stalls (`sleep 30`).
3. When the marker appears, the runner SIGKILLs the session worker
   process (found via /proc: the only process holding the trial's daemon
   socket in env/cmdline).
4. The supervisor classifies `WORKER_SOCKET_CLOSED` as an AMBIGUOUS
   delivery (fix `2b3d74842`): the prompt is NOT re-sent, the draft is
   not resubmitted over the partially executed turn.

## Invariant
`counter.txt` contains exactly one SIDE-EFFECT-ONCE line and the
transcript shows one execution of the side-effect cell. Re-execution
after worker death is the #3 P0 regression this task exists to catch.

## Missing seam (honest status)
The runner does not yet drive ACP JSON-RPC sessions (create + turn +
mid-run kill). The task, oracle, kill hook and faux script are complete;
wiring the ACP client is the remaining repo-side hole (README "Missing
seams" tracks it).
