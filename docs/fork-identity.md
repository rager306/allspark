# Allspark fork: namespace separation from upstream prime-agent

Purpose: the fork and upstream must run **in the same environment without any
shared state**. Everything below is renamed with NO fallback to the upstream
name, so a running `prime-agent` session can never leak into `allspark`.

## Why no fallback

An earlier design kept `ALLSPARK_*` as "internal names" and relied on
separate directories. That fails when `allspark` is launched *inside* a
`prime-agent` session: the inherited `ALLSPARK_CODING_AGENT_DIR` /
`ALLSPARK_DAEMON_SOCKET` / `ALLSPARK_INTERNAL_*` would point the fork at
the upstream daemon and data. Therefore every shared namespace is renamed
outright; upstream values are ignored, not read as fallback.

## Namespaces renamed (complete inventory)

| # | Namespace | Upstream | Fork | Sites | Notes |
|---|---|---|---|---|---|
| 1 | Public env | `ALLSPARK_*` (71 names) | `ALLSPARK_*` | ~585 | all user-facing knobs |
| 2 | Legacy/PI env | `PI_*`, `PI_PACKAGE_DIR`, `ALLSPARK_OFFLINE` | `ALLSPARK_*` | ~70 | `PI_PACKAGE_DIR` → `ALLSPARK_PACKAGE_DIR` |
| 3 | Internal IPC env | `ALLSPARK_INTERNAL_*` | `ALLSPARK_INTERNAL_*` | ~25 | daemon/worker plumbing |
| 4 | Daemon sockets | `/tmp/prime-agent-<uid>/…` | `/tmp/allspark-<uid>/…` | ~45 | launcher + worker + supervisor |
| 5 | Install prefix | `~/.local/share/prime-agent*` | `~/.local/share/allspark*` | ~20 | releases, launcher |
| 6 | Install marker | `share/allspark/.allspark-install` | `share/allspark/.allspark-install` | ~12 | channel marker file |
| 7 | ACP meta namespace | `ai.allspark.agent` | `ai.allspark.agent` | ~8 | wire `_meta` key |
| 8 | Trace provider id | `prime-agent-traces` | `allspark-traces` | ~10 | auth provider id |
| 9 | Supervisor registry | `~/.allspark/supervisor-owners` | `~/.allspark/supervisor-owners` | ~4 | legacy admission |
| 10 | Config/data dir | `~/.prime/agent` | `~/.allspark` | — | already done (identity) |
| 11 | Binary name | `prime-agent` | `allspark` | — | already done |
| 12 | tmp child dirs | `/tmp/allspark-rlm-*` | `/tmp/allspark-rlm-*` | ~2 | subagent scratch |

## NOT renamed (deliberate)

| Item | Reason |
|---|---|
| `PRIME_API_KEY` | Credential for the **Prime Inference** provider (a real third-party service). Renaming it would break the actual auth flow; it is not product plumbing. |
| `DO_NOT_TRACK` | Cross-tool standard (consented telemetry opt-out), not ours. |
| `models_generated.rs`, provider ids (`prime-inference`, `github-copilot`, …) | Model-catalog wire data. |
| `rlm.*` kernel API, `pa-*` crate names | Internal API surface; renaming breaks skills and merges. |
| Session JSONL format | Byte-compatible; lets you point the fork at upstream data explicitly if ever needed. |

## Where the rename lives (for clean upstream merges)

- `crates/pa-types/src/identity.rs` — the single source: `ENV_PREFIX`,
  `PACKAGE_DIR_ENV`, `META_NAMESPACE`, `TRACE_PROVIDER_ID`, `SOCKET_DIR_PREFIX`,
  `INSTALL_PREFIX_NAME`, `INSTALL_MARKER`, `LAUNCHER_NAME`.
- Every former `ALLSPARK_*` constant now resolves to an `ALLSPARK_*` value.
- `scripts/check-fork-identity.sh` — post-merge guard; fails on any new
  `ALLSPARK_*` / `PI_*` / `.prime` / `prime-agent` string outside the
  allowlist (which pins `PRIME_API_KEY` and `DO_NOT_TRACK`).

## Updating from upstream (procedure)

1. `git fetch upstream && git merge upstream/main` (or a release tag).
2. Conflicts are confined to: `identity.rs`, the ~9 env-constant definitions,
   the socket/install/meta constant sites. `git rerere` resolves repeats.
3. Run `./scripts/check-fork-identity.sh` — it lists every new upstream site
   that hardcodes an upstream name. Rename those to the identity constants.
4. `cargo fmt && cargo test -p pa-types -p pa-cli -p pa-core --lib`.
5. Rebuild: `cargo build --profile dist -p pa-cli`.

## Verification checklist (same environment, no cross-talk)

```sh
# 1. Fork ignores inherited upstream env:
ALLSPARK_CODING_AGENT_DIR=/tmp/upstream-x allspark status   # must use ~/.allspark, not /tmp/upstream-x
# 2. Sockets do not collide:
allspark status & prime-agent status &   # distinct /tmp/allspark-* and /tmp/prime-agent-*
# 3. Both run simultaneously without touching each other's data dirs.
```
