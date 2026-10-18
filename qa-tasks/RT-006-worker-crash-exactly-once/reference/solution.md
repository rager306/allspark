# Reference: RT-006 worker-crash-exactly-once

## Scenario (daemon lane)
1. The runner spawns `prime-agent --mode daemon --daemon-socket
   <sandbox>/daemon.sock`, reads `daemon_hello`, and creates + attaches a
   session whose create config carries the task's `faux-script.json`.
2. The scripted first turn appends `SIDE-EFFECT-ONCE` to `counter.txt`
   and touches `side-effect.done`, then stalls in a slow `python_repl`
   cell.
3. When the marker appears, the runner SIGKILLs the session worker pid
   taken from the durable descriptor
   (`<agent-dir>/daemon-workers/<key>/<workerId>.json`) and waits for the
   descriptor to publish the supervisor's relaunch pid.
4. The supervisor must classify the death as an AMBIGUOUS delivery (fix
   `2b3d74842`): the prompt is NOT re-sent, the partially executed turn is
   not resubmitted.

## Invariant
`counter.txt` holds exactly one `SIDE-EFFECT-ONCE` line and the
transcript holds exactly one delivery of the task prompt. Re-execution
after worker death is the #3 P0 regression this task exists to catch.

## Current status: RED (2026-10-18, measured)
The driver is wired (`--lane daemon`), the kill hook fires, and the
oracle fails the run - the relaunched worker REPLAYS the queued prompt.
Measured on `feat/isolated-defaults`:

* the worker's recovery journal records `prompt_accepted` (busy) before
  the kill;
* the supervisor relaunches the worker from the durable create command;
* `create` restores the queue from the journal
  (`worker/queue.rs::restore_queue_snapshot`) and the restored steering
  item is executed as a fresh turn;
* `counter.txt` ends with TWO `SIDE-EFFECT-ONCE` lines, the transcript
  with two deliveries of the prompt, and a `prompt_and_wait` issued
  before the kill answers `Daemon worker socket closed` - the ambiguous
  answer `2b3d74842` teaches the TUI client to treat as "may have
  executed", which the replay then contradicts.

Reproduced with NO client attached (disconnect after the marker, then
kill): the duplicate is the worker's own recovery path, not a client
retry. The fix belongs in the restore path (a restored item whose
previous delivery is journaled as admitted-and-unsettled must not be
re-executed blindly), which is product code outside this task's scope.

## Solvability (proven)
With the kill hook disabled the same task passes end-to-end (2 tool
turns, 32.7k tokens, exit 0, `SIDE-EFFECT-ONCE` once), so the oracle is
satisfiable by a correct system and the budget is not the gate. The red
is the behavior, not the harness.
