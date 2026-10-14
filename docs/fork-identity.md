# Allspark fork: isolated-by-default coexistence with prime-agent

The fork and upstream `prime-agent` run in the same environment. Isolation is
achieved by **defaults, not renames**: the fork never reads the upstream
`PRIME_AGENT_*` environment variables, and every default path is its own.

## Why the earlier full-rename was reverted

A wholesale rename (~870 sites, 71 env names) collided with upstream on every
merge forever, and broke error reporting (bug reports against `allspark` logs
are not reproducible on upstream code). The defaults-only approach keeps the
fork's diff minimal so upstream merges apply cleanly.

## Isolation model

| Resource | Upstream (`prime-agent`) | Fork (`allspark`) |
|---|---|---|
| Config/data dir | `~/.prime/agent` | `~/.allspark` |
| Binary | `~/.local/bin/prime-agent` | `~/.local/bin/allspark` |
| Runtime sockets | `/tmp/prime-agent-<uid>/` | `/tmp/allspark-<uid>/` |
| Supervisor registry | `~/.prime/supervisor-owners` | `~/.allspark/supervisor-owners` |
| Kernel venv | `~/.prime/agent/kernel-venv` | `~/.allspark/kernel-venv` |
| Env overrides | `PRIME_AGENT_*` | `ALLSPARK_*` |

## The env contract (the one rule)

The fork reads ONLY `ALLSPARK_*` variables:

- `ALLSPARK_CODING_AGENT_DIR` - override the agent state dir
- `ALLSPARK_SESSION_DIR` - override the sessions dir
- `ALLSPARK_DAEMON_SOCKET` - override the daemon socket
- `ALLSPARK_PACKAGE_DIR` - override the packaged-assets dir
- `ALLSPARK_KERNEL_VENV` - override the kernel venv

Inherited `PRIME_AGENT_*` variables from an upstream session are ignored:
the fork treats them as foreign, so launching `allspark` inside a
`prime-agent` session cannot leak into the upstream's daemon or data.

`PRIME_API_KEY` is deliberately still honored: it is the credential for the
Prime Inference provider (an external service), not product plumbing.
`DO_NOT_TRACK` is the cross-tool telemetry opt-out standard.

## Upstream references kept on purpose

- `crates/pa-cli/src/update_flow/legacy_admission.rs` reads the legacy
  `~/.prime/supervisor-owners` registry to detect a running upstream install.
- `crates/pa-cli/src/lib.rs::migrate_from_legacy_once` imports
  settings/models from `~/.prime/agent` on first run (never auth.json: a
  rotating refresh token shared by two products invalidates the other's copy).
- `crates/pa-core/src/auth/prime_inference.rs` reads the upstream Prime CLI
  config `~/.prime/config.json` for Prime Inference credentials.
- `install-rust.sh`/`install.ps1` remain the upstream installers; the fork is
  installed from its release archive (or `cp target/dist/allspark
  ~/.local/bin/`) and does not take over the upstream install tree.
- `crates/pa-daemon/src/hold_refusal.rs` recognizes a running upstream
  `prime-agent` holder to produce an actionable "TypeScript product holds
  this session" refusal.

## Updating from upstream

1. `git fetch upstream && git merge upstream/main` - the fork's identity diff
   is small: `docs/fork-identity.md` (this file), the identity constants in
   `pa-cli/src/config.rs` + `pa-daemon/src/paths.rs` (paths and env names),
   and the migration helper.
2. `./scripts/check-fork-identity.sh` - fails on any upstream name that
   collides with the fork's own (env prefix, config dir, runtime dir).
3. `cargo test -p pa-types -p pa-cli -p pa-core --lib`.

## The guard

`scripts/check-fork-identity.sh` fails on:
- `PRIME_AGENT_` / `PI_[A-Z]` env names read by the fork
- `.prime/agent` paths outside the deliberate upstream references
- `/tmp/prime-agent-` runtime paths, `share/prime-agent`,
  `.prime-agent-install`, `ai.primeintellect.prime-agent`,
  `prime-agent-traces`, `prime-agent.daemon`,
  `CARGO_BIN_EXE_prime-agent`, `name = "prime-agent"` in Cargo.toml

## History

The first attempt (commits `6f679f8c8`, `45d864beb`, `52597957a` on
`feat/allspark-identity`) renamed every namespace outright and was reverted:
~870 conflict points made every upstream merge painful, and error reports
against renamed logs were not reproducible upstream. The defaults approach
keeps the same isolation with a fraction of the diff.
