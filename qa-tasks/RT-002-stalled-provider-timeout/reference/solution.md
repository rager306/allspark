# Reference: RT-002 stalled-provider-timeout

## Why the task is solvable
`every_stream_attempt_carries_a_bounded_request_timeout` (fix
`2e79330c3`) pins that every provider attempt is wrapped in a request
timeout even when a cancel signal is present. With `retry.timeoutMs:
8000` and one retry the settled worst case is ~2x8s + backoff, far below
the 75s trial budget; without the bound the trial is killed at the
wall-clock timeout and graded red by the budget invariant.

## Oracle rationale
Duration bound + "the mock saw the request" (the request really reached
the provider before the stall). No answer-text oracle: the expected
outcome is an error outcome, not prose.
