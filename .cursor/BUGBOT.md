# Review rules

These rules mirror the repo standards in AGENTS.md - update both whenever
either changes.

## No legacy or migration scaffolding

- When a migration or refactor completes within a PR, its staging scaffolding must be deleted, not kept: no type aliases where two names point at one type, no re-exports "for compatibility" without a live external consumer, no migration-era vocabulary ("staged", "legacy", "new-style") surviving in names, comments, or test titles.
- Flag any parameter, option type, or constant that has exactly one legal value at every call site. That is ceremony, not configuration.
- Flag exported symbols, fields, and methods with zero non-test consumers. Speculative surface is worse than dead code in security-relevant modules: it implies invariants that do not exist.
- Flag predicates that are provably equivalent to an existing check, and call sites that check the same condition twice under two spellings.

## Comments

- Comments must be short, human-readable, and only where the code cannot say it. Reject comments that restate code, narrate control flow, or describe deleted mechanisms or history.
- Doc comments must describe current behavior, not the history of how it got there.

## Style

- Inline format args: flag `format!("{}", x)` and `write!(f, "{}", x)` where `format!("{x}")` compiles.
- Flag an `if` whose body is a single `if` (clippy::collapsible_if) and a closure that only forwards its argument to a method (clippy::redundant_closure_for_method_calls).
- Flag a `match` that ends in a `_ =>` wildcard arm where the enumerated variants can be named: matches are exhaustive so a new variant fails the build instead of falling into a catch-all.
- Flag a bare `bool`/`Option` literal passed positionally at a call site (`foo(false)`, `bar(None)`): use an enum, a named method, or a newtype, or at minimum an exact `/*param_name*/` comment matching the callee signature.
- Flag `#[async_trait]` and `#[allow(async_fn_in_trait)]` on new traits: the house form is a native RPITIT method with an explicit `Send` bound (`fn foo(&self) -> impl Future<Output = T> + Send;`); implementations may use `async fn` when they satisfy it.
- Flag `.instrument(...)` at a call site: async work is instrumented at its definition (`#[tracing::instrument(...)]`), and the first check is whether the callee is already instrumented.

## Prompt-input caps

- Flag any new path that reads a file into the prompt (context files, prompt-input files, skill bodies, model-prompt layers) without a byte cap and a reported truncation. A prompt is model input and cache payload at once, so an unbounded read is both a denial of service and a permanent cost; new paths reuse `truncate_context_content` and the existing `MAX_CONTEXT_FILE_BYTES` / `MAX_SKILLS_INVENTORY*` ceilings.

## Performance and the critical path

- Flag anything user-visible that awaits the network, disk sync, or background bookkeeping on a render path (telemetry, catalog fetch, update check): the paint path hands the work to a background worker and returns, and delivery semantics live with the worker.
- Flag telemetry that is awaited, batched, or flushed from startup, session create, tab open, or first-render code: delivery is fire-and-forget inside `pa-telemetry`'s background worker.
- Flag a fire-and-forget boundary with no red-first hanging-sink test: a sink that never replies must not delay paint, and a mocked or offline test cannot prove it (the #3288 regression passed every CI check because `DO_NOT_TRACK=1` and the benchmark mocked the sink).

## Branding

- Flag "pi"/"pi-mono"/"pi-ai"/"Prime Intellect" naming on user-visible surfaces — docs, READMEs, CLI help text, error messages, splash/onboarding strings, keybinding hints, TUI labels, and comments that quote user-facing strings. The product is Prime Agent.
- Wire identifiers that must stay byte-compatible with the TS product are exempt and stay until the TS side renames them: `PI_*` env vars (`PI_PACKAGE_DIR`), settings keys, provider ids such as `prime-inference`, harness `_meta` namespaces, and lockfile names. Parity beats branding on the wire.

## Adoption telemetry

- Flag a user-visible feature that ships without its adoption telemetry event in the same PR: the event name and properties join the versioned schema in `pa-telemetry` and a seam emits it from day one.
- Flag telemetry properties carrying prompt, session, or file content: primitives only, plus one level of primitive-valued map where the schema declares `PrimitiveMap`, never nested objects.

## Live-system safety

- Flag any code path, test, script, or documented experiment that runs `prime-agent update`, `allspark update`, `install-rust.sh`, or `install.sh`. The installer stops daemons by socket path, so running it from inside a live session ends the session and every worker under it (2026-10-21 incident). An update flow is proved only with a fake channel and a socket under a temp directory — never $TMPDIR/prime-agent*, $TMPDIR/allspark*, or /tmp/prime-agent*.
- Flag `env -i` used as isolation around an installer or daemon probe: it strips `PRIME_AGENT_INTERNAL_DAEMON_*`, which is exactly what the self-socket refusal reads, so it makes a destructive probe more likely.
- Flag any change to the stop-candidate set that is not proved on both install layouts (upstream Rust root and fork `bin/`+`libexec/` tarball), and flag a probe that stops a socket it cannot attribute to this install. The pinned rust socket `$TMPDIR/prime-agent-rust-<uid>/daemon.sock` is shared by both products.
- Flag commands that signal a process they did not start (`pkill -f prime-agent`, `killall`) or that remove a socket the run did not create.

## Parallel agents share one tree

- Flag instructions or scripts that commit with `git commit -a`, `git add -A`, or `git add .`, and flag any `git checkout -- <path>`, `git stash`, `git reset --hard`, `git restore`, or `git rebase` while other work is uncommitted: this checkout is worked by several agents on one branch, one index, one target dir. Commits use `git commit -o -- <path>...`.
- Flag an agent run that builds with more than `CARGO_BUILD_JOBS=2`, runs `cargo clean`, or changes the artifact key (`CARGO_PROFILE_*`, feature or rustflags override) so the tree builds a second, disjoint set of artifacts beside the one it already has.
- Flag a target sweep that runs during a build, that treats `target/debug/.cargo-lock` as a liveness signal by existence alone (cargo leaves the file behind, so only `flock -n` on it answers the question), that removes `.fingerprint`, or that reports "cleaned" without the size it freed: the point is one artifact set per build key, so the previous `Cargo.toml` version's generation and stale `<crate>-<hash>.<ext>` variants go, while the current generation stays.

## Module organization

- One concern per module, named for what it owns. Reject new `utils.rs`/`common.rs` grab-bag modules.
- A new concern lands as a new module beside its siblings (for example `crates/pa-core/src/kernel/manager/` or `crates/pa-core/src/cron/store/`), not as more lines in an already-large orchestration file; when a PR grows such a file, suggest splitting by responsibility.
- When code moves into a new module, its related tests and docs move with it.
- Prefer private modules with an explicitly exported crate API; internals are `pub(crate)`. New traits need doc comments explaining their role and how implementations are expected to behave.

## File size (advisory)

- Flag new or grown Rust files past ~2,000 lines (tests included) as advisory guidance: cite the growth and suggest splitting by responsibility. Size is ADVISORY per the operator's ruling - a soft cap that informs review and never blocks a merge; the repo's `codebase-health` PR check reports the same as warnings only.
- Generated files are exempt from the size rule (see Generated files).

## Dependency direction

- Workspace dependencies flow one way, from the composition root (`pa-cli`) toward lower crates. Flag cycles, reverse edges, or new edges that contradict the Crates table in AGENTS.md.
- `pa-types` is the only shared vocabulary crate: flag domain types duplicated into or re-exported from higher crates.
- A new dependency (workspace edge or third-party crate) is an architectural change: flag it unless the PR states why existing crates cannot host the functionality, commits the regenerated `Cargo.lock` in the same change, and keeps `make deny` clean.
- Flag cross-crate `pub use`/re-exports that widen the public API surface. `pa-tui` renders from wire types and must not link the session engine (its only workspace dependency is `pa-types`); `pa-cli` wires crates but is not wiring-only (its print-family runtime, `daemon_discovery/` and `update_flow/` are transport; `crates/pa-cli/src/lib.rs` states the boundary)

## Change hygiene

- One logical change per PR: flag PRs that mix a feature with unrelated lint fixes or refactors, and fixes that do not ride with the change that introduced them.
- If a change forces edits across many crate internals, the crate boundary is what needs fixing: flag that instead of the edits.
- Risky raw APIs route through the in-house wrappers, not new raw call sites: process control (signals/process groups), file locking, permissions, and shell selection go through `pa-core`'s `platform` wall; durable renames go through `rename_onto`; TS-parity hashing (tool-call id normalization) uses `pa-ai`'s in-house `short_hash`; environment-variable resolution goes through its owning module (telemetry overrides in `pa-telemetry`, provider API keys in `pa-ai`), not duplicated ad-hoc parsing. Flag a raw call site that bypasses these walls.

## Repo hygiene

- Flag scripts and tools that no CI workflow, Makefile target, or other code runs, such as one-off review, parity, evidence, or reproduction scripts (for example a 520-line TS-vs-Rust shutdown comparison script added in a feature PR). Nothing keeps them working; their results belong in the PR body, not in the repo.
- Flag test files that nothing runs. Before flagging, check the globs, loops, and paths built at run time in `.github/workflows/` and the Makefile (for example, ci.yml runs `scripts/test_check_<scenario>.py` through a path built inside its loop over `scripts/check_*.py`). A file name that appears nowhere can still run. Flag only when no runner, glob, or loop reaches the file; such a test only looks like coverage.
- Flag committed artifacts that no code or test reads: evidence logs, reports, generated outputs, and local venvs or caches.
- Flag a PR body that presents a script or run that CI does not run as lasting proof (for example "covered by `scripts/foo.py`"). One-off evidence such as parity-diff evidence belongs in the body, but it must read as a one-off run with its command and result, not as coverage.

## Tests

- A red unit in `scripts/fork/prepush-gate.sh` is classified, never retried to green: product regression, wrong test, or a calibration that belongs in the serial tail with its reason stated at the test. Flag a loosened assertion that exists only to make a load-sensitive test pass, and flag a serial-tail addition without a reason at the test.
- A bug fix that changes meaningful behavior lands with a regression test that fails without the fix; flag tests that pass without it. Trivial or documentation-only changes need no test.
- Reject incidental or duplicate coverage, tests that re-check behavior the PR does not change, and copy/string snapshot assertions that merely restate labels or descriptions; assert text only when the text is the behavior.
- Flag test code far out of proportion to the product change (for example 2,300 test lines for ~400 product lines). Ask for one discriminating test per behavior change, not several tests that pin the same behavior.
- Tests must wait for observable readiness: flag fixed sleeps, polling loops, and retry-to-green wrappers used as readiness signals. A timeout may bound failure; it must not make the test pass.
- Flag skipped or disabled tests without a reason stated at the test; flag tests that should use isolated ports and temporary paths but do not, and tests that leave shared state unrestored.

## Formal verification (advisory)

- Flag any attempt to make a Verus proof a merge gate, a `cargo test` target, or a CI job. `bash scripts/verus-check.sh` is advisory: it skips cleanly without the toolchain, and a proof that does not close must never block a merge.
- Every file under `verus/` is a **mirror** of a shipped function, not the function itself; flag a mirror without its `// MIRROR OF: <source> (<what it mirrors>)` header, and flag a new shared constant that is not added to the `MIRRORS` table in `scripts/verus-mirror-check.py`.
- Flag `#[verifier::external_body]` without a comment naming what it trusts and why vstd cannot express it.
- Flag a spec left looking finished when an obligation does not close: the failing obligation must be named at the file, and a spec that resists 30 attempts is abandoned rather than grown.
- Flag changes to shipped code made only to make it provable; the mirror is abandoned instead, or the invariant is stated as a test.

## Lint discipline

- Zero-warning posture: the merge gates run clippy at `-D warnings`, so any new warning is a merge blocker. Flag new warnings and any suppression added to hide one.
- Every inline `#[allow(...)]` or lint exception states its reason in a comment next to it; flag unexplained or broad exceptions. Advisory ignores in `deny.toml` need a reason and a review date.
- New lint families land whole-site: enabling a family requires fixing every current site in the same change. Flag partial enablements.

## Generated files

- The model catalog is hand-maintained data: `crates/pa-ai/src/models_generated.rs` mirrors the TS `packages/ai/src/models.generated.ts` object literal (no generator, no generated JSON — edit it by hand exactly like its TS source), and the parity fixture `crates/pa-models/tests/fixtures/catalog.v1.json` refreshes with `scripts/generate-catalog-fixture.py` from the real `PrimeIntellect-ai/prime-agent-catalog` snapshot. Flag edits that would reintroduce a generator or hand-tweak the fixture rows.
- Bulk data files are exempt from the size guidance; the exemption's reason is that the file mirrors an upstream data source.
