# Roster fixture recovery — final native result

**PASS:** focused roster outer25passed/0failed/1ignored (4.31s), adoption1/1 (0.02s), full pa-daemon lib outer1162passed/0failed/8ignored (173.27s), fmt0, all-targets scoped clippy0. Final queue exit0. Nested re-exec child1/1 is separate from outer counts. Source frozen; Cargo released. Safe structured receipt: `.research/ci-roster-native-validation.json` (exact commands/env/exits/source hashes). No fullworkspace or TS parity claim; parent owns final full make gate.

## Cause and final scope
Fixtures append under their OWN agent_dir/sessions, but `paths::sessions_dir` honors ambient PRIME_AGENT_SESSION_DIR. Default seeding therefore read a different ledger. Presence only was checked; no ambient value printed or directory contents accessed.

Existing shared roster_fixture moved to supervisor::tests, where private ledger-cache access belongs. It injects an owned temp ledger. Adoption fixture caches SAMEArc it appends to. cfg(test) supervisor tests module is pub(crate); seed tests/passivation directly import existing fixture from new owner. No production field visibility/API/helper/dependency/timeout changes. Existing actions/assertions/counts preserved.

Saved-delete constructs a fresh writer ledger from process override, bypassing reader cache. ONLY that existing test re-execs exact self once; child removes ONLY sessions-dir override and gets local marker. Parent asserts status success; child runs original complete body. No global env mutation, config clearing, retry or sleep. Original body identity checked before format.

## Causal evidence and condensed failure history
Historical root pa-daemon lib:1151passed/11failed/8ignored,143.62s; common empty roster/no publishes. Raw `.research/ci-remaining-workspace-tests.log` stays LOCAL ONLY.

Controlled command-only override points to OWN new `.research/ci-roster-override-fixture` (do not commit runtime fixture). Inverted only2cache patches: seed exact RED101 (0vs1publish,0.02s); adoption exact RED101 (missing hydrated family,0.01s). Safe receipts `.research/ci-roster-seed-controlled-red.log` and `.research/ci-roster-adoption-controlled-red.log`. Patches restored.

First fixed attempt failed compilation E0616 (private cache from sibling test module); NO TESTS RAN. Receipt `.research/ci-roster-filter-green.log` is failed despite its old name. Moved existing fixture to owner rather than widening production field or adding accessor.

Next compiled focused run was24passed/1failed/1ignored,5.97s: saved-delete fresh writer still used override. Frozen failed receipt `.research/ci-roster-filter-final-green.log`; not a green claim. Added only process-isolated wrapper above, then final-source queue ONCE with same owned outer override; no retry-to-green.

## Final checks
Every command Rust1.98.1 with PA_TS_BINARY=/nonexistent/ts, CARGO_PROFILE_DEV_DEBUG=0, CARGO_PROFILE_TEST_DEBUG=0, CARGO_BUILD_JOBS=2 and command-scoped owned override. Tests and lint use project's native Cargo interface.
- focused `.research/ci-roster-isolated-filter-green.log`:25/0/1,exit0.
- adoption `.research/ci-roster-isolated-adoption-green.log`:1/0,exit0.
- full daemon lib:1162/0/8,exit0. Raw `.research/ci-roster-isolated-daemon-lib-green.log` stays LOCAL ONLY; hash in JSON receipt.
- `.research/ci-roster-isolated-fmt.log`:exit0.
- `.research/ci-roster-isolated-clippy.log`:all-targets exit0.

## Frozen source SHA-256
- `crates/pa-daemon/src/supervisor.rs`: `e3728ec8badadc71c3f79ccdbdba9cd22bbf980d514ba26f03ea972ee6239f5d`
- `crates/pa-daemon/src/supervisor/tests.rs`: `5c914a6ec53a8c4a6956458b5799883544c9e1803bf9ecb8ba50d2c3483f24ae`
- `crates/pa-daemon/src/supervisor_roster_seed.rs`: `e46ff752f5ca93e48060b497c5d2318b6ec166b9f3c75f8109c5ddc6e7c13785`
- `crates/pa-daemon/src/supervisor_roster/passivation.rs`: `9d4846e17134467030e25ae5d2f00731632bd6f343c7f7c2d47377272cc26b9f`

No settings/auth/identity/staging/commit/push/cleanup changes by worker. No further Cargo jobs or source edits.
