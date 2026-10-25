# Decision log

Numbered decisions from the fork. Each entry: number, decision, evidence (commit or measurement), date.

| # | decision | evidence | date |
|---|----------|----------|------|
| D1 | Build identity is content-derived: `build.rs` in pa-cli stamps the short git commit via `ALLSPARK_BUILD_HASH`, injected into `daemon_hello` through `configure_build_hash` | `85c46975a`; `crates/pa-cli/build.rs`; `crates/pa-types/src/daemon/outbound.rs` | 2026-10-25 |
| D2 | Daemon-attachment compatibility is decided by ONE predicate (`daemon_attach_compat`), not by per-callsite field comparisons. Fail-closed: a missing stamp on either side is `Unknown`, not a match | `3449ba879`; `crates/pa-cli/src/interactive_mode/daemon.rs` | 2026-10-25 |
| D3 | Dynamic model discovery is driven by a YAML/JSON dictionary (`doc/discovery.json`), one row per provider. Adding a provider is a data edit, not a Rust change | `86b89e702`; `crates/pa-models/src/discovery.rs`; `doc/discovery.json` | 2026-10-25 |
| D4 | The release fetches the real catalog (`--network`), not the fixture. The fixture REPLACES the compiled catalog at runtime (chain.rs step 2), so synthetic data must never ship | `317cfc4a1`; measured: fork.010 shipped 46/46 fixture- ids | 2026-10-25 |
| D5 | `muse-spark-1.1/1.2` reject `reasoning_effort: "max"` (400 with the supported list). `MUSE_THINKING_LEVELS_NO_MAX` carries the 5-level table for those rows; 1.3+ keep the full table | live measurement 2026-10-25, api.meta.ai | 2026-10-25 |
| D6 | `.graphignore` is the repository's own ignore file for the graph, gitignore syntax, AND-ed with `respect_gitignore`. The rule file is excluded from the walk itself | `430aece2c`; `crates/pa-graph/src/indexer.rs` | 2026-10-25 |
| D7 | LSP line-addressing resolves through the server's documentSymbol tree (innermost symbol on the line), not column 0. Falls back to line start when no symbol starts there | `d5e4fb430`; `crates/pa-lsp/src/tool.rs` | 2026-10-25 |
| D8 | Shell discipline: `rg` instead of `grep`, `sd` instead of `sed`. Recorded because a regex-rewrite spliced a doc block into an unrelated test and broke clippy | `docs/shell-discipline.md` | 2026-10-25 |
| D9 | The test contract (ADR-0001) has 23 rows, 15 checks, and an FSM. The governor runs FIRST in the prepush gate (before fmt/clippy), because a matrix error costs ~7 s there | `f187b7ecf`; `scripts/testgov/` | 2026-10-25 |
