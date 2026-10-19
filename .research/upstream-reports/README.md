# Upstream reports package — PrimeIntellect-ai/prime-agent

Draft reports only. **Nothing has been submitted.** Each `.md` is a ready-to-file issue (or, for #3447/#3312, a reopen/comment draft). All findings were verified against **both eras** — the last TS release `v0.9.8` (tag `7d442aafa`, in-repo: `git show v0.9.8:…`) and the Rust port (`main` == this fork's base; working tree at `375ad95cf`). Evidence lines and proposed patches come from the fork's working tree (`git diff` of the uncommitted fixes).

## How upstream accepts reports (important)

From the fork's contribution policy (`.research/contributions-policy.md`, confirmed by CONTRIBUTING.md + code):

1. You **cannot** open a direct Issue — it is auto-closed and redirected to Discussions.
2. You **cannot** open a PR without an invitation — it is auto-closed.
3. **Discussions → [Bug reports]** is the only entry point. A maintainer creates the Issue from an accepted Discussion; only then does a vouch/PR follow.
4. The vouch bot auto-closes un-vouched issues with "issue reporters must be vouched … will be closed automatically". Of 15 closed issues across the seven review groups, only `#3363` was closed as COMPLETED — the rest are NOT_PLANNED (12 of them by the vouch bot).

**Consequence:** file these as **Discussion [Bug reports]** posts (or as comments on the existing open issues), each carrying the reproduction + evidence + patch proposal below, and expect to help until vouched. Do **not** open bare issues/PRs.

## Files and send priority

| # | File | Kind | Severity | Upstream | Priority |
|---|---|---|---|---|---|
| 1 | `issue-3447-reopen-kernel-otrunc.md` | reopen + patch | **P1** | `#3447` (NOT_PLANNED), `#3082`, `#3346` | **1 — send first** |
| 2 | `issue-new-pkce-state.md` | new (security) | P1/P2 | new (mirrors codex flow) | **2** |
| 3 | `issue-new-traceback-caps.md` | new | **P1** | new | **3** |
| 4 | `issue-new-provider-stream-hang.md` | new (reliability) | **P1** | new | **4** |
| 5 | `issue-new-atfile-unbounded.md` | new | P2 | new | **5** |
| 6 | `issue-new-image-token-estimate.md` | new | P2 | new | **6** |
| 7 | `issue-3312-armed-route-failover.md` | comment on open issue + recipe | P2 | `#3312` (**OPEN**) | **7** |
| 8 | `issue-new-providerbackupmodel-unread.md` | new | P3 | new (adjacent `#3312`) | **8** |

### Why this order

1. **`#3447` (P1, silent data loss).** The only irreversible one: a session resume truncates a user file opened for writing while reporting success. A ready patch + test already exist, and the issue was closed by a bot without a fix — reopen with the patch. Start here.
2. **PKCE `state` (P1 formally).** Broken PKCE on the Anthropic login; trivially fixed and independently contradicted by Claude Code's own client (binary evidence). High signal, low patch risk.
3. **Traceback caps (P1).** Megabyte exception reprs are re-billed every turn; a local clip at settle restores the output-budget invariant. Strong cost/effect ratio.
4. **Provider stream hang (P1).** Live hangs (three in one day); the fix is the already-correct bedrock `h2.rs` pattern. Reliability, easy to demonstrate.
5. **`@file` unbounded (P2).** Clear reproduction, easy patch; needs an explicit parity-divergence note (the TS binary embedded whole files).
6. **Image token estimate (P2).** Real trailing-window under-count; smaller blast radius (compaction mistiming), and the fix is a deliberate divergence.
7. **`#3312` (P2, already open).** Comment on the open issue with our analysis + the third-party PR #24 recipe; do **not** file a duplicate. Coordinate, since the current behaviour is pinned by a comment + test.
8. **`providerBackupModel` unread (P3).** Dead setting; small, but rides along with `#3312`.

## Cross-cutting notes

- **"Closed" ≠ fixed.** Every report states the issue-tracker state explicitly, because 14/15 closed issues in this area are NOT_PLANNED (12 by the vouch bot), not fixed.
- **Parity divergences.** Items 2, 5, 6 (PKCE, `@file`, image tokens) change behaviour relative to TS v0.9.8 on purpose; each report says so and carries a parity note, so a reviewer can accept or reject the divergence knowingly.
- **Shared runtime.** Item 1's patch is in `prime-agent-runtime/src/rlm/repl.py`, which is byte-identical to upstream `main` — the same patch applies upstream unchanged.
- **Source research** (do not duplicate; read for detail): `.research/verify-ts-era.md`, `verify-security.md`, `verify-unbounded.md`, `verify-upstream-wire.md`, `verify-reliability.md`.
- **Patches** are the uncommitted `git diff` in the working tree; the key fragment of each is quoted in the corresponding report.
