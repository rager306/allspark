# Reference: RT-012 failover-402-backup

## Why the task is solvable
`retry.failover` + the #4713 payment re-route (`pa-core
session_engine/provider_failover.rs`: "a payment failure re-routes while
another provider serves the same model"; unit tests
`payment_failure_switches_immediately_and_succeeds`,
`payment_failure_walks_the_chain_until_candidates_run_out`) hand a 402
primary to the next provider serving the same model id. The e2e corpus
`crates/pa-daemon/tests/provider_failover_e2e.rs` proves the chain walk
against exactly this two-mock models.json layout.

## Lane finding (2026-10-17, empirical)
The re-route driver lives in the DAEMON turn runner
(`pa-daemon/src/agent_engine/turn/run_once.rs` reads the retry/failover
policies; the code even states "Faux-script sessions never fail over").
The daemonless `-p` print path aborts the turn on the payment error
without consulting the chain - verified live: primary 402 -> turn fails,
backup mock saw 0 requests. Hence this task is `daemon` lane: it needs
the ACP/daemon driver (README "Missing seams"), same as RT-006.

## Reference behavior (daemon lane)
1. POST primary/chat/completions -> 402 (classified insufficient_credits).
2. Payment failure spends the provider budget at once; the chain re-routes.
3. POST backup/chat/completions -> SSE completion `BACKUP-ANSWER-7f3a`.
4. The turn completes on the backup; the mock saw >= 1 backup request.
