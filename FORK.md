<!-- FORK-ONLY: upstream merge must not overwrite this file -->
# This repository is the allspark fork

A fork of [PrimeIntellect-ai/prime-agent](https://github.com/PrimeIntellect-ai/prime-agent);
upstream is described by `README.md`. This file is the fork's orientation map.

## What the fork adds

Seven fork-only crates, each marked `FORK-ONLY` in its `src/lib.rs`. Lines are lines of
Rust under `src/`, recursively, tests inside `src/` included:

| crate | lines | purpose |
| --- | ---: | --- |
| `pa-graph` | 30,969 | on-disk SQLite code graph: FTS5 search, call/import edges, coverage |
| `pa-edit` | 25,444 | the `edit` engine: replace / patch / apply_patch / hashline / sloppy |
| `pa-lsp` | 9,024 | native LSP tool: symbol navigation, diagnostics, symbol-level edits |
| `pa-ast` | 5,798 | tree-sitter + ast-grep parsing, used by `pa-graph`, `pa-lsp`, `pa-edit` |
| `pa-mem` | 5,661 | RAM layout primitives ported safely from pogocache (sharded maps, snapshots) |
| `pa-jev` | 1,325 | typed-judgment wire types and client (`choice` / `score` / `noul`) |
| `pa-diff` | 1,110 | Myers line/word diff, dependency-free; `pa-edit` renders previews with it |

79,331 lines in total. Beyond these crates the fork adds providers, `debugLog`, tool-surface
profiles and rate-limit-aware admission; `readme-fork.md` covers that.

## Where the work lives

- Tags: `allspark-v<version>-fork.NNN`, zero-padded, `fork.001`..`fork.009`; newest
  `allspark-v0.10.1-beta.38-fork.009`. Twelve `allspark*` tags exist: those nine plus
  `-fork.1`/`-fork.2` (only on the abandoned `feat/allspark-identity` branch) and `-fork.3`
  (on `main`, same commit as `fork.002`, no changelog entry). Take `fork.009` as the release.
- Build stamp: `ALLSPARK_FORK_BUILD=fork.NNN cargo build --profile dist --bin prime-agent`.
- The binary keeps the upstream name `prime-agent` **on purpose**, so upstream merges and
  TS-parity diffs stay clean. The user-facing entry is `allspark`
  (`scripts/allspark-launcher.sh`, deployed as `bin/allspark`, payload at
  `libexec/prime-agent`).
- Isolation is one knob: the launcher points `HOME` at `~/.allspark-home` and pins the
  daemon socket to `$TMPDIR/allspark-daemon.sock`. Nothing is read from `~/.prime`.

## How to tell you are looking at the fork

```sh
allspark --version
# allspark <version> (fork build fork.NNN; upstream prime-agent <version>)
```

An unstamped build prints `fork build dev`; the line is in `crates/pa-cli/src/lib.rs`, the id
is `option_env!("ALLSPARK_FORK_BUILD")`. Check it: `git ls-tree -d --name-only upstream/main
crates/` lists none of the seven crates.

## Where to read more

| file | answers |
| --- | --- |
| `readme-fork.md` | everything the fork adds, in detail |
| `CHANGELOG-FORK.md` | per-release history, fork.001 to fork.009 |
| `docs/fork-identity.md` | namespaces, isolation model, known deviations |
| `.research/INDEX.md` | the research corpus, ordered by the decision it carries |
| `AGENTS.md` | development rules; its "Fork map" is the agent-facing version of this file |
