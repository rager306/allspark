<!-- FORK-ONLY: upstream merge must not overwrite this file -->
# Allspark fork identity: launcher isolation, upstream payload names

The fork and upstream `prime-agent` coexist in one environment. This document
describes the isolation model **as implemented** — the contract the code and
the launcher actually honor. The earlier "defaults-not-renames" plan
(`~/.allspark` defaults, `ALLSPARK_*` env, legacy migration) was superseded;
none of it is in the code. Where this file and `AGENTS.md` (root) disagree,
`AGENTS.md` wins.

## The model in one line

**The payload keeps upstream names; isolation is one knob: `HOME`.**

The user-facing entry is the `allspark` launcher
(`scripts/allspark-launcher.sh`, deployed as `bin/allspark` with the payload
at `libexec/prime-agent`). The launcher points `HOME` at an isolated home and
pins the daemon socket; every product path (sessions, auth, settings, logs,
kernel venv, skills) derives from `$HOME`, so a co-installed upstream shares
nothing with the fork.

## Why payload names stay upstream (by design)

- **Upstream merges**: the tree keeps upstream names (`prime-agent` bin,
  `pa-*` crates, `PRIME_AGENT_*` env), so `git merge upstream/main` applies
  cleanly. The abandoned in-binary rebrand (`feat/allspawn-identity`,
  re-land attempt `feat/allspark-identity-reland`) dragged ~700 rename sites
  into every future merge and broke the launcher's socket pin — keep it
  unmerged.
- **TS parity**: wire-internal identifiers must stay byte-compatible with
  the TypeScript product (`PRIME_AGENT_*`/`PI_*` env, settings keys, provider
  ids, `_meta` namespaces, lockfile names). Parity beats branding on the wire.
- **Error reporting**: bug reports and parity diffs reference upstream names.

## What the launcher pins

| Knob | Value | Effect |
|---|---|---|
| `HOME` | `$ALLSPARK_HOME` (default `~/.allspark-home`) | all `$HOME`-derived state moves under the isolated home |
| `PRIME_AGENT_CODING_AGENT_DIR` | `$ALLSPARK_HOME/.prime/agent` | an inherited upstream override cannot point the fork at upstream state |
| `PRIME_AGENT_DAEMON_SOCKET` | `${TMPDIR:-/tmp}/allspark-daemon.sock` (respects a pre-set value) | the fork daemon never fights the upstream daemon on the product-default socket |

**No seeding**: nothing under `~/.prime` is ever read or copied. The fork
starts standalone — own providers catalog, settings, auth (`allspark login`).
`auth.json` is upstream-only in particular: rotating refresh tokens shared by
two products invalidate each other.

## Isolation table (actual)

| Resource | Upstream | Fork (via launcher) |
|---|---|---|
| Config/data dir | `~/.prime/agent` | `$ALLSPARK_HOME/.prime/agent` |
| Sessions/logs/venv | under `~/.prime/agent` | under `$ALLSPARK_HOME/.prime/agent` |
| Daemon socket | `$TMPDIR/prime-agent-<uid>/daemon.sock` | `$TMPDIR/allspark-daemon.sock` |
| Binary | `~/.local/bin/prime-agent` | `bin/allspark` (launcher) + `libexec/prime-agent` (payload) |
| Version string | `prime-agent <v>` | `allspark <v> (fork build <id>; upstream prime-agent <v>)` |

Env names are **shared** (`PRIME_AGENT_*`): the collision is avoided by the
isolated `HOME`, not by renaming. Renaming buys nothing and breaks the
launcher's pins plus parity diffs.

## Known deviations and open edges

- **Unix runtime dir is fork-named**: worker sockets live under
  `/tmp/allspark-<uid>/` (`pa-daemon/src/platform/paths.rs`), while the
  supervisor socket comes from the launcher pin. Consequence (known bug #22):
  the installer/update stop-probe list must include the fork socket so a fork
  update never kills the upstream daemon (and vice versa).
- **Windows named pipes keep upstream names** (TS-parity, pinned by
  `windows_endpoints_are_the_ts_pipe_names`): fork and upstream share the
  pipe namespace on Windows. Open question — decide the Windows model before
  shipping a Windows fork build (issue #2 in the CE review).
- **`migrate_from_legacy_once` is dead code** (`pa-cli/src/lib.rs`, reachable
  only from its test): it implements the superseded `~/.allspark` migration.
  Either delete it or wire it behind an explicit flag; as shipped it is
  misleading.
- **`legacy_admission`** reads the upstream `~/.prime/supervisor-owners`
  registry to *detect* a running upstream install (refusal UX only) —
  read-only, by design.

## Identity guard (contract, not yet implemented)

`scripts/check-fork-identity.sh` does not exist yet. When added, it must
assert, from a populated upstream `~/.prime`:

1. `bin/allspark` run via the launcher writes nothing under the real
   `~/.prime` and contacts no upstream socket;
2. the payload run **without** the launcher warns (or refuses on
   state-writing commands) that it will use upstream defaults;
3. `allspark --version` carries the fork build id matching the release tag
   `allspark-v<version>-fork.NNN`.

## Verifying isolation after changes

Re-run the recipe above against a populated upstream `~/.prime` (create one
with the upstream binary, then run the fork launcher) and confirm: no new
entries under `~/.prime`, no connection to `$TMPDIR/prime-agent-<uid>/`.
Any change to the launcher, payload name, or isolation variables must state
so in its PR and re-verify this property.

## Updating from upstream

1. `git fetch upstream && git merge upstream/main` — the identity model above
   exists to keep this clean;
2. resolve conflicts keeping this file and `AGENTS.md` (fork sections) intact;
3. rebuild with `ALLSPARK_FORK_BUILD=fork.NNN cargo build --profile dist -p pa-cli`
   and tag `allspark-v<version>-fork.NNN` (zero-padded fork counter);
4. re-verify isolation (previous section).
